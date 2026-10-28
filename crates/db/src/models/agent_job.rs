use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

use super::execution_process::RunUsageDelta;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AgentJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

/// A headless agent job without a repo (D34): intake or history analysis.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct AgentJob {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub kind: String,
    pub agent_def_id: Option<String>,
    pub model: Option<String>,
    pub status: AgentJobStatus,
    pub container_id: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub tokens_in: i64,
    #[ts(type = "number")]
    pub tokens_out: i64,
    #[ts(type = "number")]
    pub tokens_cache_read: i64,
    #[ts(type = "number")]
    pub tokens_cache_write: i64,
    pub cost_usd: f64,
    #[ts(type = "number")]
    pub model_calls: i64,
    pub budget_usd: Option<f64>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub const JOB_KIND_INTAKE: &str = "intake";
pub const JOB_KIND_HISTORY: &str = "history";

impl AgentJob {
    pub async fn create(
        pool: &SqlitePool,
        kind: &str,
        project_id: Option<Uuid>,
        issue_id: Option<Uuid>,
        agent_def_id: &str,
        model: &str,
        budget_usd: Option<f64>,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO agent_jobs (id, project_id, issue_id, kind, agent_def_id, model,
                                       budget_usd)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            id,
            project_id,
            issue_id,
            kind,
            agent_def_id,
            model,
            budget_usd
        )
        .execute(pool)
        .await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            AgentJob,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id: Uuid",
                      issue_id as "issue_id: Uuid",
                      kind,
                      agent_def_id,
                      model,
                      status as "status!: AgentJobStatus",
                      container_id,
                      output,
                      error,
                      tokens_in,
                      tokens_out,
                      tokens_cache_read,
                      tokens_cache_write,
                      cost_usd,
                      model_calls,
                      budget_usd,
                      started_at as "started_at: DateTime<Utc>",
                      completed_at as "completed_at: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>"
               FROM agent_jobs
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn set_running(
        pool: &SqlitePool,
        id: Uuid,
        container_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE agent_jobs
               SET status = 'running', container_id = $2,
                   started_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            container_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn finish(
        pool: &SqlitePool,
        id: Uuid,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let status = if error.is_some() {
            AgentJobStatus::Failed
        } else {
            AgentJobStatus::Completed
        };
        sqlx::query!(
            r#"UPDATE agent_jobs
               SET status = $2, output = $3, error = $4,
                   completed_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            status,
            output,
            error
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Adds one model call's usage. Returns false when `id` is not a job
    /// (the proxy's run ids are execution processes otherwise).
    pub async fn add_usage(
        pool: &SqlitePool,
        id: Uuid,
        delta: &RunUsageDelta,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE agent_jobs
               SET tokens_in = tokens_in + $2,
                   tokens_out = tokens_out + $3,
                   tokens_cache_read = tokens_cache_read + $4,
                   tokens_cache_write = tokens_cache_write + $5,
                   cost_usd = cost_usd + $6,
                   model_calls = model_calls + 1
               WHERE id = $1"#,
            id,
            delta.tokens_in,
            delta.tokens_out,
            delta.tokens_cache_read,
            delta.tokens_cache_write,
            delta.cost_usd
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Jobs left running by a previous launch can never finish.
    pub async fn fail_interrupted(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE agent_jobs
               SET status = 'failed', error = 'Interrupted by an app restart',
                   completed_at = datetime('now', 'subsec')
               WHERE status IN ('queued', 'running')"#
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
