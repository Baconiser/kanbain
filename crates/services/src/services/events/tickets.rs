//! Live updates for the local ticket board.
//!
//! Every row change in a ticket table becomes a JSON-Patch on the shared
//! message store, at `/<collection>/<id>`. Streams filter those patches by
//! project (or issue, for comments) and start with a snapshot.

use std::sync::Arc;

use db::models::{
    issue::Issue,
    issue_comment::IssueComment,
    issue_relationship::IssueRelationship,
    project::Project,
    project_status::ProjectStatus,
    project_tag::{IssueTag, ProjectTag},
};
use futures::StreamExt;
use json_patch::{AddOperation, Patch, PatchOperation, RemoveOperation, ReplaceOperation};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::{SqlitePool, sqlite::SqliteOperation};
use tokio_stream::wrappers::BroadcastStream;
use utils::{log_msg::LogMsg, msg_store::MsgStore};
use uuid::Uuid;

use super::{EventService, types::EventError};

/// Ticket tables that emit live updates, with their patch collection names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketTable {
    Projects,
    ProjectStatuses,
    Issues,
    ProjectTags,
    IssueTags,
    IssueRelationships,
    IssueComments,
}

impl TicketTable {
    pub fn from_table(table: &str) -> Option<Self> {
        Some(match table {
            "projects" => Self::Projects,
            "project_statuses" => Self::ProjectStatuses,
            "issues" => Self::Issues,
            "project_tags" => Self::ProjectTags,
            "issue_tags" => Self::IssueTags,
            "issue_relationships" => Self::IssueRelationships,
            "issue_comments" => Self::IssueComments,
            _ => return None,
        })
    }

    /// The top-level key in the client document, and the patch path prefix.
    pub fn collection(self) -> &'static str {
        match self {
            Self::Projects => "projects",
            Self::ProjectStatuses => "statuses",
            Self::Issues => "issues",
            Self::ProjectTags => "tags",
            Self::IssueTags => "issue_tags",
            Self::IssueRelationships => "relationships",
            Self::IssueComments => "comments",
        }
    }
}

/// Collections that make up a project board stream.
const BOARD_TABLES: &[TicketTable] = &[
    TicketTable::ProjectStatuses,
    TicketTable::Issues,
    TicketTable::ProjectTags,
    TicketTable::IssueTags,
    TicketTable::IssueRelationships,
];

fn path(table: TicketTable, id: Uuid) -> String {
    format!("/{}/{}", table.collection(), id)
}

pub fn upsert_patch<T: Serialize>(table: TicketTable, id: Uuid, value: &T, insert: bool) -> Patch {
    let path = path(table, id).try_into().expect("ticket path is valid");
    let value = serde_json::to_value(value).expect("ticket row serializes");
    Patch(vec![if insert {
        PatchOperation::Add(AddOperation { path, value })
    } else {
        PatchOperation::Replace(ReplaceOperation { path, value })
    }])
}

pub fn remove_patch(table: TicketTable, id: Uuid) -> Patch {
    Patch(vec![PatchOperation::Remove(RemoveOperation {
        path: path(table, id).try_into().expect("ticket path is valid"),
    })])
}

/// A ticket row change seen by the SQLite hooks, held until its transaction
/// commits so streams never see uncommitted or rolled-back state.
#[derive(Debug, Clone)]
pub enum PendingChange {
    /// Inserted or updated row, re-read after commit.
    Upsert {
        table: TicketTable,
        rowid: i64,
        insert: bool,
    },
    /// Deleted row; the id comes from the pre-update hook.
    Remove { table: TicketTable, id: Uuid },
}

impl PendingChange {
    pub fn from_update(
        table: TicketTable,
        rowid: i64,
        operation: &SqliteOperation,
    ) -> Option<Self> {
        match operation {
            SqliteOperation::Insert => Some(Self::Upsert {
                table,
                rowid,
                insert: true,
            }),
            SqliteOperation::Update => Some(Self::Upsert {
                table,
                rowid,
                insert: false,
            }),
            _ => None,
        }
    }
}

/// Pushes patches for committed changes, in commit order.
pub async fn flush_changes(
    pool: &SqlitePool,
    msg_store: &Arc<MsgStore>,
    changes: Vec<PendingChange>,
) {
    for change in changes {
        match change {
            PendingChange::Upsert {
                table,
                rowid,
                insert,
            } => {
                if let Err(e) = push_row_change(pool, msg_store, table, rowid, insert).await {
                    tracing::error!("Failed to push ticket update: {:?}", e);
                }
            }
            PendingChange::Remove { table, id } => {
                msg_store.push_patch(remove_patch(table, id));
            }
        }
    }
}

/// Loads the changed row and pushes an add/replace patch.
async fn push_row_change(
    pool: &SqlitePool,
    msg_store: &Arc<MsgStore>,
    table: TicketTable,
    rowid: i64,
    insert: bool,
) -> Result<(), sqlx::Error> {
    let patch = match table {
        TicketTable::Projects => Project::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::ProjectStatuses => ProjectStatus::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::Issues => Issue::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::ProjectTags => ProjectTag::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::IssueTags => IssueTag::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::IssueRelationships => IssueRelationship::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
        TicketTable::IssueComments => IssueComment::find_by_rowid(pool, rowid)
            .await?
            .map(|r| upsert_patch(table, r.id, &r, insert)),
    };
    if let Some(patch) = patch {
        msg_store.push_patch(patch);
    }
    Ok(())
}

fn to_map<T: Serialize>(rows: Vec<T>, id: impl Fn(&T) -> Uuid) -> Value {
    let map: Map<String, Value> = rows
        .into_iter()
        .map(|row| {
            (
                id(&row).to_string(),
                serde_json::to_value(row).expect("ticket row serializes"),
            )
        })
        .collect();
    Value::Object(map)
}

/// Whether a patch belongs to a stream: its collection is in `tables` and,
/// for add/replace, the row's `scope_field` equals `scope_id`. Removes always
/// pass; ids are unique, so clients ignore removes for rows they don't have.
fn patch_matches(patch: &Patch, tables: &[TicketTable], scope: Option<(&str, Uuid)>) -> bool {
    let Some(op) = patch.0.first() else {
        return false;
    };
    let in_collection = tables.iter().any(|t| {
        op.path()
            .to_string()
            .starts_with(&format!("/{}/", t.collection()))
    });
    if !in_collection {
        return false;
    }
    let value = match op {
        PatchOperation::Add(a) => &a.value,
        PatchOperation::Replace(r) => &r.value,
        PatchOperation::Remove(_) => return true,
        _ => return false,
    };
    match scope {
        None => true,
        Some((field, scope_id)) => value
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|v| v == scope_id.to_string()),
    }
}

impl EventService {
    fn filtered_ticket_stream(
        &self,
        initial: Value,
        tables: &'static [TicketTable],
        scope: Option<(&'static str, Uuid)>,
    ) -> futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>> {
        // One replace per collection; clients may not support a root replace.
        let ops: Vec<Value> = initial
            .as_object()
            .expect("initial snapshot is an object")
            .iter()
            .map(|(key, value)| json!({ "op": "replace", "path": format!("/{key}"), "value": value }))
            .collect();
        let initial_msg = LogMsg::JsonPatch(
            serde_json::from_value(Value::Array(ops)).expect("initial patch is valid"),
        );

        let live = BroadcastStream::new(self.msg_store.get_receiver()).filter_map(move |msg| {
            let result = match msg {
                Ok(LogMsg::JsonPatch(patch)) if patch_matches(&patch, tables, scope) => {
                    Some(Ok(LogMsg::JsonPatch(patch)))
                }
                _ => None,
            };
            async move { result }
        });

        futures::stream::iter(vec![Ok(initial_msg), Ok(LogMsg::Ready)])
            .chain(live)
            .boxed()
    }

    /// All projects, as `{ projects: { <id>: Project } }`.
    pub async fn stream_projects_raw(
        &self,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, EventError>
    {
        let projects = Project::find_all(&self.db.pool).await?;
        let initial = json!({ "projects": to_map(projects, |p| p.id) });
        Ok(self.filtered_ticket_stream(initial, &[TicketTable::Projects], None))
    }

    /// One project's board: statuses, issues, tags, issue tags and
    /// relationships, each keyed by id.
    pub async fn stream_project_board_raw(
        &self,
        project_id: Uuid,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, EventError>
    {
        let pool = &self.db.pool;
        let initial = json!({
            "statuses": to_map(ProjectStatus::find_by_project(pool, project_id).await?, |r| r.id),
            "issues": to_map(Issue::find_by_project(pool, project_id).await?, |r| r.id),
            "tags": to_map(ProjectTag::find_by_project(pool, project_id).await?, |r| r.id),
            "issue_tags": to_map(IssueTag::find_by_project(pool, project_id).await?, |r| r.id),
            "relationships": to_map(
                IssueRelationship::find_by_project(pool, project_id).await?,
                |r| r.id
            ),
        });
        Ok(self.filtered_ticket_stream(initial, BOARD_TABLES, Some(("project_id", project_id))))
    }

    /// One issue's comments, as `{ comments: { <id>: IssueComment } }`.
    pub async fn stream_issue_comments_raw(
        &self,
        issue_id: Uuid,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, EventError>
    {
        let comments = IssueComment::find_by_issue(&self.db.pool, issue_id).await?;
        let initial = json!({ "comments": to_map(comments, |c| c.id) });
        Ok(self.filtered_ticket_stream(
            initial,
            &[TicketTable::IssueComments],
            Some(("issue_id", issue_id)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_patch_filtering() {
        let project = Uuid::new_v4();
        let other = Uuid::new_v4();
        let row = json!({ "id": Uuid::new_v4(), "project_id": project });
        let foreign = json!({ "id": Uuid::new_v4(), "project_id": other });
        let scope = Some(("project_id", project));

        let ok = upsert_patch(TicketTable::Issues, Uuid::new_v4(), &row, true);
        assert!(patch_matches(&ok, BOARD_TABLES, scope));

        let wrong_project = upsert_patch(TicketTable::Issues, Uuid::new_v4(), &foreign, false);
        assert!(!patch_matches(&wrong_project, BOARD_TABLES, scope));

        let comment = upsert_patch(TicketTable::IssueComments, Uuid::new_v4(), &row, true);
        assert!(!patch_matches(&comment, BOARD_TABLES, scope));

        let removed = remove_patch(TicketTable::ProjectTags, Uuid::new_v4());
        assert!(patch_matches(&removed, BOARD_TABLES, scope));
    }
}
