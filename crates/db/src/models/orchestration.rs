//! The orchestrator's state (Phase 5, D22): a ticket's coding run and one
//! row per plan subtask. Everything the scheduler needs lives here, so a
//! restart picks up where the last launch stopped.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type, types::Json};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum OrchestrationStatus {
    /// Subtasks are running or waiting for their turn.
    Running,
    /// You stopped it, or stopped subtasks leave nothing to run.
    Stopped,
    /// A subtask failed; the ticket is in Blocked.
    Blocked,
    /// Every subtask is merged into the ticket branch.
    Done,
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SubtaskStatus {
    /// Waiting for its dependencies, an overlapping subtask or a slot.
    Pending,
    Running,
    /// Its branch is merged into the ticket branch.
    Merged,
    Failed,
    Stopped,
}

/// The last merge conflict of a subtask; its re-run gets this as context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SubtaskConflict {
    /// `repo/path` of every conflicted file.
    pub files: Vec<String>,
    /// The earlier attempt's changes (capped), to re-apply on the new base.
    pub previous_diff: String,
}

/// A ticket's coding run.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct TicketOrchestration {
    pub issue_id: Uuid,
    pub plan_id: Uuid,
    /// The workspace holding the ticket branch.
    pub workspace_id: Option<Uuid>,
    pub status: OrchestrationStatus,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One subtask of the approved plan.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct OrchestrationSubtask {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub plan_subtask_id: String,
    #[ts(type = "number")]
    pub position: i64,
    pub sub_issue_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub execution_process_id: Option<Uuid>,
    pub status: SubtaskStatus,
    #[ts(type = "number")]
    pub fix_round: i64,
    #[ts(type = "SubtaskConflict | null")]
    pub conflict: Option<Json<SubtaskConflict>>,
    /// Merge commit per repo name.
    #[ts(type = "Record<string, string> | null")]
    pub merge_commits: Option<Json<BTreeMap<String, String>>>,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl TicketOrchestration {
    pub async fn find(pool: &SqlitePool, issue_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            TicketOrchestration,
            r#"SELECT issue_id as "issue_id!: Uuid",
                      plan_id as "plan_id!: Uuid",
                      workspace_id as "workspace_id: Uuid",
                      status as "status!: OrchestrationStatus",
                      error,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM ticket_orchestrations
               WHERE issue_id = $1"#,
            issue_id
        )
        .fetch_optional(pool)
        .await
    }

    /// Tickets whose coding was running when the last launch ended.
    pub async fn find_running(pool: &SqlitePool) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT issue_id as "issue_id!: Uuid" FROM ticket_orchestrations
               WHERE status = 'running'"#
        )
        .fetch_all(pool)
        .await
    }

    /// Starts (or restarts) a ticket's coding run for `plan_id`.
    pub async fn upsert(
        pool: &SqlitePool,
        issue_id: Uuid,
        plan_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO ticket_orchestrations (issue_id, plan_id, workspace_id)
               VALUES ($1, $2, $3)
               ON CONFLICT (issue_id) DO UPDATE
                   SET plan_id = $2, workspace_id = $3, status = 'running', error = NULL,
                       updated_at = datetime('now', 'subsec')"#,
            issue_id,
            plan_id,
            workspace_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_status(
        pool: &SqlitePool,
        issue_id: Uuid,
        status: OrchestrationStatus,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE ticket_orchestrations SET status = $2, error = $3,
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
}

impl OrchestrationSubtask {
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            OrchestrationSubtask,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      plan_subtask_id,
                      position,
                      sub_issue_id as "sub_issue_id: Uuid",
                      workspace_id as "workspace_id: Uuid",
                      execution_process_id as "execution_process_id: Uuid",
                      status as "status!: SubtaskStatus",
                      fix_round,
                      conflict as "conflict: Json<SubtaskConflict>",
                      merge_commits as "merge_commits: Json<BTreeMap<String, String>>",
                      error,
                      started_at as "started_at: DateTime<Utc>",
                      finished_at as "finished_at: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM orchestration_subtasks
               WHERE issue_id = $1
               ORDER BY position"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            OrchestrationSubtask,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      plan_subtask_id,
                      position,
                      sub_issue_id as "sub_issue_id: Uuid",
                      workspace_id as "workspace_id: Uuid",
                      execution_process_id as "execution_process_id: Uuid",
                      status as "status!: SubtaskStatus",
                      fix_round,
                      conflict as "conflict: Json<SubtaskConflict>",
                      merge_commits as "merge_commits: Json<BTreeMap<String, String>>",
                      error,
                      started_at as "started_at: DateTime<Utc>",
                      finished_at as "finished_at: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM orchestration_subtasks
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    /// The subtask a coder run belongs to.
    pub async fn find_by_run(
        pool: &SqlitePool,
        execution_process_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let id = sqlx::query_scalar!(
            r#"SELECT id as "id!: Uuid" FROM orchestration_subtasks
               WHERE execution_process_id = $1"#,
            execution_process_id
        )
        .fetch_optional(pool)
        .await?;
        match id {
            Some(id) => Self::find_by_id(pool, id).await,
            None => Ok(None),
        }
    }

    /// The subtask working in a workspace.
    pub async fn find_by_workspace(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let id = sqlx::query_scalar!(
            r#"SELECT id as "id!: Uuid" FROM orchestration_subtasks
               WHERE workspace_id = $1"#,
            workspace_id
        )
        .fetch_optional(pool)
        .await?;
        match id {
            Some(id) => Self::find_by_id(pool, id).await,
            None => Ok(None),
        }
    }

    pub async fn create(
        pool: &SqlitePool,
        issue_id: Uuid,
        plan_subtask_id: &str,
        position: i64,
        sub_issue_id: Option<Uuid>,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO orchestration_subtasks (id, issue_id, plan_subtask_id, position,
                                                   sub_issue_id)
               VALUES ($1, $2, $3, $4, $5)"#,
            id,
            issue_id,
            plan_subtask_id,
            position,
            sub_issue_id
        )
        .execute(pool)
        .await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Drops a ticket's subtasks (a newly approved plan replaces them).
    pub async fn delete_by_issue(pool: &SqlitePool, issue_id: Uuid) -> Result<u64, sqlx::Error> {
        Ok(sqlx::query!(
            "DELETE FROM orchestration_subtasks WHERE issue_id = $1",
            issue_id
        )
        .execute(pool)
        .await?
        .rows_affected())
    }

    pub async fn set_workspace(
        pool: &SqlitePool,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET workspace_id = $2,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            workspace_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Claims a pending subtask for a coder run. False when it isn't
    /// pending any more (stopped meanwhile, or already claimed).
    pub async fn claim(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'running', error = NULL,
                   execution_process_id = NULL, started_at = datetime('now', 'subsec'),
                   finished_at = NULL, updated_at = datetime('now', 'subsec')
               WHERE id = $1 AND status = 'pending'"#,
            id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_run(
        pool: &SqlitePool,
        id: Uuid,
        execution_process_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET execution_process_id = $2,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            execution_process_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Sets the status; `finished_at` marks the end of a run.
    pub async fn set_status(
        pool: &SqlitePool,
        id: Uuid,
        status: SubtaskStatus,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let finished = matches!(
            status,
            SubtaskStatus::Merged | SubtaskStatus::Failed | SubtaskStatus::Stopped
        );
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = $2, error = $3,
                   finished_at = CASE WHEN $4 THEN datetime('now', 'subsec') ELSE finished_at END,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            status,
            error,
            finished
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_merged(
        pool: &SqlitePool,
        id: Uuid,
        merge_commits: &BTreeMap<String, String>,
    ) -> Result<(), sqlx::Error> {
        let commits = Json(merge_commits);
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'merged', merge_commits = $2,
                   error = NULL, finished_at = datetime('now', 'subsec'),
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            commits
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Records a merge conflict and queues the re-run (D22).
    pub async fn set_conflict(
        pool: &SqlitePool,
        id: Uuid,
        conflict: &SubtaskConflict,
    ) -> Result<(), sqlx::Error> {
        let conflict = Json(conflict);
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'pending', conflict = $2,
                   fix_round = fix_round + 1, finished_at = datetime('now', 'subsec'),
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            conflict
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Back to pending for Retry; keeps the branch and the fix rounds.
    pub async fn requeue(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'pending', error = NULL,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1 AND status IN ('failed', 'stopped')"#,
            id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Coder runs of a previous launch can never finish; their subtasks of
    /// running tickets go back to pending so the scheduler starts them again.
    pub async fn requeue_interrupted(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'pending',
                   updated_at = datetime('now', 'subsec')
               WHERE status = 'running'
                 AND issue_id IN (SELECT issue_id FROM ticket_orchestrations
                                  WHERE status = 'running')"#
        )
        .execute(pool)
        .await?;
        // A stopped ticket's interrupted run counts as stopped.
        sqlx::query!(
            r#"UPDATE orchestration_subtasks SET status = 'stopped',
                   error = 'Interrupted by an app restart',
                   updated_at = datetime('now', 'subsec')
               WHERE status = 'running'"#
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
