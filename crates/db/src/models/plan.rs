//! Plans (D20, D21): the planner's `plan.json`, versioned per ticket, and a
//! ticket's planning state.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// Ids in plans are strings; models like to write `1` instead of `"1"`.
fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        S(String),
        N(serde_json::Number),
    }
    Ok(match Id::deserialize(d)? {
        Id::S(s) => s,
        Id::N(n) => n.to_string(),
    })
}

fn strings_or_numbers<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    struct Wrap(#[serde(deserialize_with = "string_or_number")] String);
    Ok(Vec::<Wrap>::deserialize(d)?
        .into_iter()
        .map(|w| w.0)
        .collect())
}

/// One unit of work; Phase 5 turns each into a sub-ticket and a coder run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PlanSubtask {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Files the subtask likely touches; disjoint sets may run in parallel.
    #[serde(default)]
    pub files: Vec<String>,
    /// Ids of subtasks that must finish first.
    #[serde(default, deserialize_with = "strings_or_numbers")]
    pub depends_on: Vec<String>,
}

/// Where the tester runs (Phase 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PlanCheckpoint {
    #[serde(default, deserialize_with = "strings_or_numbers")]
    pub after_subtasks: Vec<String>,
    pub what: String,
}

/// The `plan.json` schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct PlanDocument {
    pub subtasks: Vec<PlanSubtask>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub test_strategy: String,
    #[serde(default)]
    pub checkpoints: Vec<PlanCheckpoint>,
}

impl PlanDocument {
    /// Parses and validates a plan. Errors are sentences the planner (or
    /// the user) can act on.
    pub fn from_value(value: serde_json::Value) -> Result<Self, Vec<String>> {
        let plan: Self = serde_json::from_value(value).map_err(|e| vec![e.to_string()])?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.subtasks.is_empty() {
            errors.push("The plan has no subtasks.".to_string());
        }
        let mut ids = HashSet::new();
        for (i, subtask) in self.subtasks.iter().enumerate() {
            let n = i + 1;
            if subtask.id.trim().is_empty() {
                errors.push(format!("Subtask {n} has no id."));
            } else if !ids.insert(subtask.id.as_str()) {
                errors.push(format!("Subtask id `{}` is used twice.", subtask.id));
            }
            if subtask.title.trim().is_empty() {
                errors.push(format!("Subtask {n} has no title."));
            }
        }
        for subtask in &self.subtasks {
            for dep in &subtask.depends_on {
                if dep == &subtask.id {
                    errors.push(format!("Subtask `{}` depends on itself.", subtask.id));
                } else if !ids.contains(dep.as_str()) {
                    errors.push(format!(
                        "Subtask `{}` depends on `{dep}`, which is not a subtask.",
                        subtask.id
                    ));
                }
            }
        }
        if errors.is_empty()
            && let Some(id) = self.cycle()
        {
            errors.push(format!("The dependencies of subtask `{id}` form a cycle."));
        }
        for (i, checkpoint) in self.checkpoints.iter().enumerate() {
            if checkpoint.what.trim().is_empty() {
                errors.push(format!("Checkpoint {} doesn't say what to test.", i + 1));
            }
            for id in &checkpoint.after_subtasks {
                if !ids.contains(id.as_str()) {
                    errors.push(format!(
                        "Checkpoint {} comes after `{id}`, which is not a subtask.",
                        i + 1
                    ));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// A subtask on a dependency cycle, if there is one.
    fn cycle(&self) -> Option<&str> {
        let deps: HashMap<&str, Vec<&str>> = self
            .subtasks
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.depends_on.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        // 1 = on the current path, 2 = done.
        fn visit<'a>(
            id: &'a str,
            deps: &HashMap<&'a str, Vec<&'a str>>,
            state: &mut HashMap<&'a str, u8>,
        ) -> bool {
            match state.get(id) {
                Some(1) => return true,
                Some(_) => return false,
                None => {}
            }
            state.insert(id, 1);
            for dep in deps.get(id).into_iter().flatten() {
                if visit(dep, deps, state) {
                    return true;
                }
            }
            state.insert(id, 2);
            false
        }
        let mut state = HashMap::new();
        self.subtasks
            .iter()
            .map(|s| s.id.as_str())
            .find(|id| visit(id, &deps, &mut state))
    }

    /// The plan as Markdown, for search and comments.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("## Subtasks\n");
        for s in &self.subtasks {
            out.push_str(&format!("\n### {}. {}\n", s.id, s.title));
            if !s.description.trim().is_empty() {
                out.push_str(&format!("{}\n", s.description.trim()));
            }
            if !s.files.is_empty() {
                out.push_str(&format!("Files: {}\n", s.files.join(", ")));
            }
            if !s.depends_on.is_empty() {
                out.push_str(&format!("Depends on: {}\n", s.depends_on.join(", ")));
            }
        }
        let list = |out: &mut String, title: &str, items: &[String]| {
            if !items.is_empty() {
                out.push_str(&format!("\n## {title}\n"));
                for item in items {
                    out.push_str(&format!("- {item}\n"));
                }
            }
        };
        list(&mut out, "Acceptance criteria", &self.acceptance_criteria);
        list(&mut out, "Assumptions", &self.assumptions);
        if !self.test_strategy.trim().is_empty() {
            out.push_str(&format!(
                "\n## Test strategy\n{}\n",
                self.test_strategy.trim()
            ));
        }
        if !self.checkpoints.is_empty() {
            out.push_str("\n## Checkpoints\n");
            for c in &self.checkpoints {
                out.push_str(&format!(
                    "- after {}: {}\n",
                    if c.after_subtasks.is_empty() {
                        "start".to_string()
                    } else {
                        c.after_subtasks.join(", ")
                    },
                    c.what
                ));
            }
        }
        out
    }
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum PlanStatus {
    Draft,
    Approved,
    Rejected,
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum PlanSource {
    Planner,
    Edited,
}

/// One version of a ticket's plan.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct Plan {
    pub id: Uuid,
    pub issue_id: Uuid,
    #[ts(type = "number")]
    pub version: i64,
    #[ts(type = "PlanDocument")]
    pub plan: sqlx::types::Json<PlanDocument>,
    pub status: PlanStatus,
    pub source: PlanSource,
    pub feedback: Option<String>,
    pub execution_process_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

impl Plan {
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Plan,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      version,
                      plan as "plan!: sqlx::types::Json<PlanDocument>",
                      status as "status!: PlanStatus",
                      source as "source!: PlanSource",
                      feedback,
                      execution_process_id as "execution_process_id: Uuid",
                      created_at as "created_at!: DateTime<Utc>",
                      decided_at as "decided_at: DateTime<Utc>"
               FROM plans
               WHERE issue_id = $1
               ORDER BY version"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Plan,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      version,
                      plan as "plan!: sqlx::types::Json<PlanDocument>",
                      status as "status!: PlanStatus",
                      source as "source!: PlanSource",
                      feedback,
                      execution_process_id as "execution_process_id: Uuid",
                      created_at as "created_at!: DateTime<Utc>",
                      decided_at as "decided_at: DateTime<Utc>"
               FROM plans
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn latest(pool: &SqlitePool, issue_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        Ok(Self::find_by_issue(pool, issue_id).await?.pop())
    }

    /// Stores the next version of a ticket's plan as a draft.
    pub async fn create(
        pool: &SqlitePool,
        issue_id: Uuid,
        plan: &PlanDocument,
        source: PlanSource,
        execution_process_id: Option<Uuid>,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let plan = sqlx::types::Json(plan);
        sqlx::query!(
            r#"INSERT INTO plans (id, issue_id, version, plan, source, execution_process_id)
               VALUES ($1, $2,
                       (SELECT COALESCE(MAX(version), 0) + 1 FROM plans WHERE issue_id = $2),
                       $3, $4, $5)"#,
            id,
            issue_id,
            plan,
            source,
            execution_process_id
        )
        .execute(pool)
        .await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn decide(
        pool: &SqlitePool,
        id: Uuid,
        status: PlanStatus,
        feedback: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE plans SET status = $2, feedback = COALESCE($3, feedback),
                   decided_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            status,
            feedback
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// Where a ticket's planning flow stands.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum PlanningStatus {
    Idle,
    /// A planner run is being prepared or is running.
    Planning,
    /// The planner asked questions; the user's reply resumes it.
    WaitingForAnswer,
    AwaitingApproval,
    Approved,
    Failed,
}

/// A ticket's planner workspace, session and state.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssuePlanning {
    pub issue_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub execution_process_id: Option<Uuid>,
    pub status: PlanningStatus,
    #[ts(type = "number")]
    pub repair_attempts: i64,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// A ticket's planning state on the board (live, scoped by project).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct BoardPlanning {
    pub issue_id: Uuid,
    pub project_id: Uuid,
    pub status: PlanningStatus,
}

impl BoardPlanning {
    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            BoardPlanning,
            r#"SELECT p.issue_id as "issue_id!: Uuid",
                      i.project_id as "project_id!: Uuid",
                      p.status as "status!: PlanningStatus"
               FROM issue_planning p JOIN issues i ON i.id = p.issue_id
               WHERE p.rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            BoardPlanning,
            r#"SELECT p.issue_id as "issue_id!: Uuid",
                      i.project_id as "project_id!: Uuid",
                      p.status as "status!: PlanningStatus"
               FROM issue_planning p JOIN issues i ON i.id = p.issue_id
               WHERE i.project_id = $1"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }
}

impl IssuePlanning {
    pub async fn find(pool: &SqlitePool, issue_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssuePlanning,
            r#"SELECT issue_id as "issue_id!: Uuid",
                      workspace_id as "workspace_id: Uuid",
                      session_id as "session_id: Uuid",
                      execution_process_id as "execution_process_id: Uuid",
                      status as "status!: PlanningStatus",
                      repair_attempts,
                      error,
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issue_planning
               WHERE issue_id = $1"#,
            issue_id
        )
        .fetch_optional(pool)
        .await
    }

    /// The planning a planner run belongs to.
    pub async fn find_by_run(
        pool: &SqlitePool,
        execution_process_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssuePlanning,
            r#"SELECT issue_id as "issue_id!: Uuid",
                      workspace_id as "workspace_id: Uuid",
                      session_id as "session_id: Uuid",
                      execution_process_id as "execution_process_id: Uuid",
                      status as "status!: PlanningStatus",
                      repair_attempts,
                      error,
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issue_planning
               WHERE execution_process_id = $1"#,
            execution_process_id
        )
        .fetch_optional(pool)
        .await
    }

    /// Claims the ticket for a new planner run. False when a run is
    /// already being prepared or running.
    pub async fn begin(pool: &SqlitePool, issue_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"INSERT INTO issue_planning (issue_id, status) VALUES ($1, 'planning')
               ON CONFLICT (issue_id) DO UPDATE
                   SET status = 'planning', error = NULL, execution_process_id = NULL,
                       updated_at = datetime('now', 'subsec')
                   WHERE issue_planning.status != 'planning'"#,
            issue_id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_run(
        pool: &SqlitePool,
        issue_id: Uuid,
        workspace_id: Uuid,
        session_id: Uuid,
        execution_process_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE issue_planning SET workspace_id = $2, session_id = $3,
                   execution_process_id = $4, updated_at = datetime('now', 'subsec')
               WHERE issue_id = $1"#,
            issue_id,
            workspace_id,
            session_id,
            execution_process_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_status(
        pool: &SqlitePool,
        issue_id: Uuid,
        status: PlanningStatus,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE issue_planning SET status = $2, error = $3,
                   updated_at = datetime('now', 'subsec')
               WHERE issue_id = $1"#,
            issue_id,
            status,
            error
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_repair_attempts(
        pool: &SqlitePool,
        issue_id: Uuid,
        attempts: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE issue_planning SET repair_attempts = $2 WHERE issue_id = $1",
            issue_id,
            attempts
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Planner runs of a previous launch can never finish.
    pub async fn fail_interrupted(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE issue_planning SET status = 'failed',
                   error = 'Interrupted by an app restart'
               WHERE status = 'planning'"#
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_numeric_ids_and_defaults() {
        let plan = PlanDocument::from_value(json!({
            "subtasks": [
                {"id": 1, "title": "Model", "files": ["a.rs"]},
                {"id": "2", "title": "Route", "depends_on": [1]}
            ],
            "checkpoints": [{"after_subtasks": [2], "what": "API works"}]
        }))
        .unwrap();
        assert_eq!(plan.subtasks[0].id, "1");
        assert_eq!(plan.subtasks[1].depends_on, vec!["1"]);
        assert_eq!(plan.checkpoints[0].after_subtasks, vec!["2"]);
        assert!(plan.acceptance_criteria.is_empty());
    }

    #[test]
    fn rejects_bad_references_duplicates_and_cycles() {
        let errors = PlanDocument::from_value(json!({
            "subtasks": [
                {"id": "a", "title": "A", "depends_on": ["x"]},
                {"id": "a", "title": ""}
            ],
            "checkpoints": [{"after_subtasks": ["zz"], "what": ""}]
        }))
        .unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("used twice")),
            "{errors:?}"
        );
        assert!(errors.iter().any(|e| e.contains("no title")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("`x`")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("`zz`")), "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("doesn't say")),
            "{errors:?}"
        );

        let errors = PlanDocument::from_value(json!({
            "subtasks": [
                {"id": "a", "title": "A", "depends_on": ["b"]},
                {"id": "b", "title": "B", "depends_on": ["a"]}
            ]
        }))
        .unwrap_err();
        assert!(errors[0].contains("cycle"), "{errors:?}");

        let errors = PlanDocument::from_value(json!({"subtasks": []})).unwrap_err();
        assert!(errors[0].contains("no subtasks"));
        assert!(PlanDocument::from_value(json!({"tasks": []})).is_err());
    }

    #[test]
    fn markdown_lists_everything() {
        let plan = PlanDocument::from_value(json!({
            "subtasks": [{"id": "1", "title": "Do it", "files": ["x.rs"]}],
            "assumptions": ["Mail goes over SMTP"],
            "test_strategy": "Unit tests",
            "checkpoints": [{"after_subtasks": ["1"], "what": "it works"}]
        }))
        .unwrap();
        let md = plan.to_markdown();
        assert!(md.contains("### 1. Do it\nFiles: x.rs"));
        assert!(md.contains("## Assumptions\n- Mail goes over SMTP"));
        assert!(md.contains("- after 1: it works"));
    }
}
