use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type, types::Json};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// Where an agent-driven rebase stands.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ConflictResolutionStage {
    /// Handed off; waiting for the resolver run to start.
    Queued,
    /// The resolver is rebasing and resolving in the sandbox.
    Rebasing,
    /// The run ended; the branch is coming back to the host.
    Syncing,
    /// The host branch sits on top of the target: ready to merge.
    Resolved,
    Failed,
}

impl ConflictResolutionStage {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Rebasing | Self::Syncing)
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ConflictResolution {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub execution_process_id: Option<Uuid>,
    pub target_branch: String,
    pub target_oid: String,
    pub branch_oid: String,
    #[ts(type = "string[]")]
    pub predicted_files: Json<Vec<String>>,
    pub stage: ConflictResolutionStage,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateConflictResolution<'a> {
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub target_branch: &'a str,
    pub target_oid: &'a str,
    pub branch_oid: &'a str,
    pub predicted_files: &'a [String],
}

impl ConflictResolution {
    /// Starts a new attempt. Any attempt still in flight for the same repo
    /// is failed first: two resolvers rebasing one branch is a mess nobody
    /// should have to untangle.
    pub async fn create(
        pool: &SqlitePool,
        data: CreateConflictResolution<'_>,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query(
            r#"UPDATE conflict_resolutions SET stage = 'failed',
                   error = 'Superseded by a newer attempt',
                   updated_at = datetime('now', 'subsec')
               WHERE workspace_id = $1 AND repo_id = $2
                 AND stage IN ('queued', 'rebasing', 'syncing')"#,
        )
        .bind(data.workspace_id)
        .bind(data.repo_id)
        .execute(pool)
        .await?;
        sqlx::query_as::<_, Self>(
            r#"INSERT INTO conflict_resolutions
                   (id, workspace_id, repo_id, target_branch, target_oid, branch_oid,
                    predicted_files)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING *"#,
        )
        .bind(Uuid::new_v4())
        .bind(data.workspace_id)
        .bind(data.repo_id)
        .bind(data.target_branch)
        .bind(data.target_oid)
        .bind(data.branch_oid)
        .bind(Json(data.predicted_files))
        .fetch_one(pool)
        .await
    }

    /// The newest attempt for each repo of a workspace.
    pub async fn latest_for_workspace(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"SELECT * FROM conflict_resolutions r
               WHERE workspace_id = $1
                 AND created_at = (SELECT MAX(created_at) FROM conflict_resolutions
                                   WHERE workspace_id = r.workspace_id
                                     AND repo_id = r.repo_id)"#,
        )
        .bind(workspace_id)
        .fetch_all(pool)
        .await
    }

    /// The attempt waiting for its run to start, if any.
    pub async fn find_queued(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"SELECT * FROM conflict_resolutions
               WHERE workspace_id = $1 AND stage = 'queued'
               ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(workspace_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_execution(
        pool: &SqlitePool,
        execution_process_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            "SELECT * FROM conflict_resolutions WHERE execution_process_id = $1",
        )
        .bind(execution_process_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn start_run(
        pool: &SqlitePool,
        id: Uuid,
        execution_process_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE conflict_resolutions SET stage = 'rebasing',
                   execution_process_id = $2, updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
        )
        .bind(id)
        .bind(execution_process_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn set_stage(
        pool: &SqlitePool,
        id: Uuid,
        stage: ConflictResolutionStage,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE conflict_resolutions SET stage = $2, error = $3,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
        )
        .bind(id)
        .bind(stage)
        .bind(error)
        .execute(pool)
        .await?;
        Ok(())
    }
}
