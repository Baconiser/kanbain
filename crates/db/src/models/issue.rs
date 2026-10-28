use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::rust::double_option;
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

use super::{
    issue_description::{DescriptionKind, IssueDescription, NewDescriptionVersion},
    project_status::{ProjectStatus, ProjectStatusKind},
};

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum IssuePriority {
    Urgent,
    High,
    Medium,
    Low,
}

/// Whether agents should ask the user (`ask`) or proceed on stated
/// assumptions (`yolo`) when something is unclear.
#[derive(
    Debug,
    Clone,
    Copy,
    Type,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    TS,
    EnumString,
    Display,
    Default,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum IssueMode {
    #[default]
    Ask,
    Yolo,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct Issue {
    pub id: Uuid,
    pub project_id: Uuid,
    #[ts(type = "number")]
    pub number: i64,
    pub status_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<IssuePriority>,
    pub sort_order: f64,
    pub parent_issue_id: Option<Uuid>,
    pub parent_issue_sort_order: Option<f64>,
    pub external_id: Option<String>,
    pub mode: IssueMode,
    #[ts(type = "number")]
    pub fix_round: i64,
    pub budget_usd: Option<f64>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Issue {
    /// The internal id, e.g. `KAN-42`.
    pub fn internal_id(&self, id_prefix: &str) -> String {
        format!("{id_prefix}-{}", self.number)
    }

    /// The id used for branches and commits: the External ID when set,
    /// otherwise the internal id.
    pub fn display_id(&self, id_prefix: &str) -> String {
        self.external_id
            .clone()
            .unwrap_or_else(|| self.internal_id(id_prefix))
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateIssue {
    pub title: String,
    #[serde(default)]
    #[ts(optional)]
    pub description: Option<String>,
    /// Defaults to the project's first Backlog status.
    #[serde(default)]
    #[ts(optional)]
    pub status_id: Option<Uuid>,
    #[serde(default)]
    #[ts(optional)]
    pub priority: Option<IssuePriority>,
    #[serde(default)]
    #[ts(optional)]
    pub sort_order: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub parent_issue_id: Option<Uuid>,
    #[serde(default)]
    #[ts(optional)]
    pub parent_issue_sort_order: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub external_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub mode: Option<IssueMode>,
    #[serde(default)]
    #[ts(optional)]
    pub budget_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct UpdateIssue {
    #[serde(default)]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "string | null")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    #[ts(optional)]
    pub status_id: Option<Uuid>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "IssuePriority | null")]
    pub priority: Option<Option<IssuePriority>>,
    #[serde(default)]
    #[ts(optional)]
    pub sort_order: Option<f64>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "string | null")]
    pub parent_issue_id: Option<Option<Uuid>>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "number | null")]
    pub parent_issue_sort_order: Option<Option<f64>>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "string | null")]
    pub external_id: Option<Option<String>>,
    #[serde(default)]
    #[ts(optional)]
    pub mode: Option<IssueMode>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "number | null")]
    pub budget_usd: Option<Option<f64>>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct BulkUpdateIssueItem {
    pub id: Uuid,
    #[serde(flatten)]
    #[ts(flatten)]
    pub changes: UpdateIssue,
}

#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Status does not belong to this project")]
    InvalidStatus,
    #[error("Parent ticket does not belong to this project")]
    InvalidParent,
    #[error("A ticket cannot be its own parent or descendant")]
    ParentCycle,
    #[error("Project has no statuses")]
    NoStatuses,
}

fn clean_text(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl Issue {
    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Issue,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      number,
                      status_id as "status_id!: Uuid",
                      title,
                      description,
                      priority as "priority: IssuePriority",
                      sort_order,
                      parent_issue_id as "parent_issue_id: Uuid",
                      parent_issue_sort_order,
                      external_id,
                      mode as "mode!: IssueMode",
                      fix_round,
                      budget_usd,
                      completed_at as "completed_at: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues
               WHERE project_id = $1
               ORDER BY sort_order ASC, number ASC"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Issue,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      number,
                      status_id as "status_id!: Uuid",
                      title,
                      description,
                      priority as "priority: IssuePriority",
                      sort_order,
                      parent_issue_id as "parent_issue_id: Uuid",
                      parent_issue_sort_order,
                      external_id,
                      mode as "mode!: IssueMode",
                      fix_round,
                      budget_usd,
                      completed_at as "completed_at: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Issue,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      number,
                      status_id as "status_id!: Uuid",
                      title,
                      description,
                      priority as "priority: IssuePriority",
                      sort_order,
                      parent_issue_id as "parent_issue_id: Uuid",
                      parent_issue_sort_order,
                      external_id,
                      mode as "mode!: IssueMode",
                      fix_round,
                      budget_usd,
                      completed_at as "completed_at: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_external_id(
        pool: &SqlitePool,
        project_id: Uuid,
        external_id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Issue,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      number,
                      status_id as "status_id!: Uuid",
                      title,
                      description,
                      priority as "priority: IssuePriority",
                      sort_order,
                      parent_issue_id as "parent_issue_id: Uuid",
                      parent_issue_sort_order,
                      external_id,
                      mode as "mode!: IssueMode",
                      fix_round,
                      budget_usd,
                      completed_at as "completed_at: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues
               WHERE project_id = $1 AND external_id = $2 COLLATE NOCASE
               ORDER BY number ASC
               LIMIT 1"#,
            project_id,
            external_id
        )
        .fetch_optional(pool)
        .await
    }

    async fn validate_status(
        pool: &SqlitePool,
        project_id: Uuid,
        status_id: Uuid,
    ) -> Result<ProjectStatus, IssueError> {
        match ProjectStatus::find_by_id(pool, status_id).await? {
            Some(status) if status.project_id == project_id => Ok(status),
            _ => Err(IssueError::InvalidStatus),
        }
    }

    /// Checks that `parent_id` is in the project and is not `issue_id` or one
    /// of its descendants.
    async fn validate_parent(
        pool: &SqlitePool,
        project_id: Uuid,
        issue_id: Option<Uuid>,
        parent_id: Uuid,
    ) -> Result<(), IssueError> {
        let mut current = Some(parent_id);
        let mut depth = 0;
        while let Some(id) = current {
            if Some(id) == issue_id {
                return Err(IssueError::ParentCycle);
            }
            let Some(issue) = Self::find_by_id(pool, id).await? else {
                return Err(IssueError::InvalidParent);
            };
            if issue.project_id != project_id {
                return Err(IssueError::InvalidParent);
            }
            current = issue.parent_issue_id;
            depth += 1;
            if depth > 64 {
                return Err(IssueError::ParentCycle);
            }
        }
        Ok(())
    }

    pub async fn create(
        pool: &SqlitePool,
        project_id: Uuid,
        data: &CreateIssue,
    ) -> Result<Self, IssueError> {
        Self::create_with_version(
            pool,
            project_id,
            data,
            &NewDescriptionVersion::plain(DescriptionKind::Original),
        )
        .await
    }

    /// Creates a ticket whose first description version is described by
    /// `version` (intake stores the dropped and enhanced texts with it).
    pub async fn create_with_version(
        pool: &SqlitePool,
        project_id: Uuid,
        data: &CreateIssue,
        version: &NewDescriptionVersion,
    ) -> Result<Self, IssueError> {
        let status = match data.status_id {
            Some(status_id) => Self::validate_status(pool, project_id, status_id).await?,
            None => {
                let statuses = ProjectStatus::find_by_project(pool, project_id).await?;
                statuses
                    .iter()
                    .find(|s| s.kind == ProjectStatusKind::Backlog)
                    .or(statuses.first())
                    .cloned()
                    .ok_or(IssueError::NoStatuses)?
            }
        };
        if let Some(parent_id) = data.parent_issue_id {
            Self::validate_parent(pool, project_id, None, parent_id).await?;
        }

        let id = Uuid::new_v4();
        let title = data.title.trim().to_string();
        let description = clean_text(data.description.clone());
        let external_id = clean_text(data.external_id.clone());
        let mode = data.mode.unwrap_or_default();
        let completed_at = (status.kind == ProjectStatusKind::Done).then(Utc::now);

        let mut tx = pool.begin().await?;
        let number = sqlx::query_scalar!(
            r#"UPDATE projects SET issue_counter = issue_counter + 1
               WHERE id = $1
               RETURNING issue_counter as "issue_counter!: i64""#,
            project_id
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!(
            r#"INSERT INTO issues (id, project_id, number, status_id, title, description,
                                   priority, sort_order, parent_issue_id,
                                   parent_issue_sort_order, external_id, mode, budget_usd,
                                   completed_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7,
                       COALESCE($8, (SELECT COALESCE(MAX(sort_order), 0) + 1
                                     FROM issues WHERE status_id = $4)),
                       $9, $10, $11, $12, $13, $14)"#,
            id,
            project_id,
            number,
            status.id,
            title,
            description,
            data.priority,
            data.sort_order,
            data.parent_issue_id,
            data.parent_issue_sort_order,
            external_id,
            mode,
            data.budget_usd,
            completed_at
        )
        .execute(&mut *tx)
        .await?;
        IssueDescription::insert_next(&mut tx, id, &title, description.as_deref(), version).await?;
        tx.commit().await?;

        Ok(Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?)
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateIssue,
    ) -> Result<Self, IssueError> {
        Self::update_with_version(pool, id, data, None).await
    }

    /// Updates a ticket. A changed description becomes a new `edited`
    /// description version; `version` forces a new version described by it
    /// (an intake update of an existing ticket).
    pub async fn update_with_version(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateIssue,
        version: Option<&NewDescriptionVersion>,
    ) -> Result<Self, IssueError> {
        let existing = Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;

        let title = data
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(&existing.title)
            .to_string();
        let description = match &data.description {
            Some(d) => clean_text(d.clone()),
            None => existing.description.clone(),
        };

        let mut completed_at = existing.completed_at;
        let status_id = match data.status_id {
            Some(status_id) if status_id != existing.status_id => {
                let status = Self::validate_status(pool, existing.project_id, status_id).await?;
                completed_at = (status.kind == ProjectStatusKind::Done).then(Utc::now);
                status_id
            }
            _ => existing.status_id,
        };

        let parent_issue_id = match data.parent_issue_id {
            Some(Some(parent_id)) => {
                Self::validate_parent(pool, existing.project_id, Some(id), parent_id).await?;
                Some(parent_id)
            }
            Some(None) => None,
            None => existing.parent_issue_id,
        };
        let priority = data.priority.unwrap_or(existing.priority);
        let sort_order = data.sort_order.unwrap_or(existing.sort_order);
        let parent_issue_sort_order = data
            .parent_issue_sort_order
            .unwrap_or(existing.parent_issue_sort_order);
        let external_id = match &data.external_id {
            Some(e) => clean_text(e.clone()),
            None => existing.external_id.clone(),
        };
        let mode = data.mode.unwrap_or(existing.mode);
        let budget_usd = data.budget_usd.unwrap_or(existing.budget_usd);

        let description_changed = description != existing.description;

        let mut tx = pool.begin().await?;
        sqlx::query!(
            r#"UPDATE issues
               SET title = $2, description = $3, status_id = $4, priority = $5,
                   sort_order = $6, parent_issue_id = $7, parent_issue_sort_order = $8,
                   external_id = $9, mode = $10, budget_usd = $11, completed_at = $12,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            title,
            description,
            status_id,
            priority,
            sort_order,
            parent_issue_id,
            parent_issue_sort_order,
            external_id,
            mode,
            budget_usd,
            completed_at
        )
        .execute(&mut *tx)
        .await?;
        match version {
            Some(meta) => {
                IssueDescription::insert_next(&mut tx, id, &title, description.as_deref(), meta)
                    .await?;
            }
            None if description_changed => {
                IssueDescription::record_edit(&mut tx, id, &title, description.as_deref()).await?;
            }
            None => {}
        }
        tx.commit().await?;

        Ok(Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?)
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!("DELETE FROM issues WHERE id = $1", id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}
