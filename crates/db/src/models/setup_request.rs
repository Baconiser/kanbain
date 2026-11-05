use chrono::{DateTime, Utc};
use executors::adapters::definition::AgentRole;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SetupRequestStatus {
    /// Waits for the user.
    Pending,
    /// An approved setup covers it.
    Resolved,
    Dismissed,
}

/// Something an agent found missing in its sandbox and asked for.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct SetupRequest {
    pub id: Uuid,
    pub project_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub execution_process_id: Option<Uuid>,
    /// Role of the run that asked; `None` when its agent is unknown.
    #[sqlx(try_from = "Option<String>")]
    #[ts(type = "AgentRole | null")]
    pub role: RoleColumn,
    pub agent: Option<String>,
    pub need: String,
    pub reason: String,
    pub suggestion: Option<String>,
    pub status: SetupRequestStatus,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// `role` as stored: snake_case text, or NULL. An unknown name (a role that
/// was renamed since) reads as `None` instead of failing the whole list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleColumn(pub Option<AgentRole>);

impl From<Option<String>> for RoleColumn {
    fn from(value: Option<String>) -> Self {
        Self(value.and_then(|v| v.parse().ok()))
    }
}

#[derive(Debug, Clone)]
pub struct CreateSetupRequest {
    pub project_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub execution_process_id: Option<Uuid>,
    pub role: Option<AgentRole>,
    pub agent: Option<String>,
    pub need: String,
    pub reason: String,
    pub suggestion: Option<String>,
}

impl SetupRequest {
    pub async fn create(pool: &SqlitePool, data: &CreateSetupRequest) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query_as::<_, Self>(
            r#"INSERT INTO setup_requests (id, project_id, issue_id, execution_process_id,
                   role, agent, need, reason, suggestion)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
               RETURNING *"#,
        )
        .bind(id)
        .bind(data.project_id)
        .bind(data.issue_id)
        .bind(data.execution_process_id)
        .bind(data.role.map(|r| r.to_string()))
        .bind(&data.agent)
        .bind(&data.need)
        .bind(&data.reason)
        .bind(&data.suggestion)
        .fetch_one(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>("SELECT * FROM setup_requests WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// The project's open requests, oldest first.
    pub async fn find_pending(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"SELECT * FROM setup_requests
               WHERE project_id = $1 AND status = 'pending'
               ORDER BY created_at"#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
    }

    /// Requests one run made so far.
    pub async fn count_for_run(pool: &SqlitePool, run_id: Uuid) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM setup_requests WHERE execution_process_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
    }

    /// Marks pending requests of `project_id` resolved or dismissed. Ids of
    /// other projects or already decided requests are skipped.
    pub async fn decide(
        pool: &SqlitePool,
        project_id: Uuid,
        ids: &[Uuid],
        status: SetupRequestStatus,
    ) -> Result<u64, sqlx::Error> {
        let mut changed = 0;
        for id in ids {
            changed += sqlx::query(
                r#"UPDATE setup_requests SET status = $3,
                       resolved_at = datetime('now', 'subsec')
                   WHERE id = $1 AND project_id = $2 AND status = 'pending'"#,
            )
            .bind(id)
            .bind(project_id)
            .bind(status)
            .execute(pool)
            .await?
            .rows_affected();
        }
        Ok(changed)
    }
}
