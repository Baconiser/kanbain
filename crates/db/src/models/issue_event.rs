use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// What a timeline entry records (D19). Most are written by triggers on the
/// ticket tables (see the Phase 3a migration); the rest by the code that
/// does the work.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, Display, EnumString,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum IssueEventType {
    Created,
    StatusChanged,
    TitleChanged,
    PriorityChanged,
    ExternalIdChanged,
    ModeChanged,
    /// A new description version (`version`, `kind`).
    DescriptionVersion,
    Comment,
    AttachmentAdded,
    /// The intake agent enhanced the dropped ticket.
    Intake,
    /// The git-history check found commits for the ticket's id (Phase 3b).
    HistoryCheck,
    /// The history agent summarized prior work (Phase 3b).
    HistoryAnalysis,
    /// A commit for the ticket's id, at its commit time (Phase 3b).
    PriorCommit,
    /// A planner run started (`reason`: assigned, answer, feedback, repair).
    PlanningStarted,
    /// The planner asked questions in the thread (Phase 4).
    PlanQuestions,
    /// A new plan version (`version`, `source`).
    PlanVersion,
    PlanApproved,
    /// The user sent a plan back with feedback.
    PlanRejected,
    /// A planner run ended without a usable plan or questions.
    PlanningFailed,
    RunStarted,
    RunFinished,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueEvent {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub event_type: IssueEventType,
    /// `user`, `agent:<job or run id>` or `system`.
    pub actor: String,
    #[ts(type = "Record<string, unknown>")]
    pub payload: sqlx::types::Json<Value>,
    /// When it happened; before the ticket existed for prior commits.
    pub occurred_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// SQLite's own `datetime('now', 'subsec')` format, so explicit times sort
/// with the defaults.
pub fn sqlite_time(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

impl IssueEvent {
    pub async fn create(
        pool: &SqlitePool,
        issue_id: Uuid,
        event_type: IssueEventType,
        actor: &str,
        payload: &Value,
        occurred_at: Option<DateTime<Utc>>,
    ) -> Result<Uuid, sqlx::Error> {
        let id = Uuid::new_v4();
        let payload = sqlx::types::Json(payload);
        let occurred_at = occurred_at.map(sqlite_time);
        sqlx::query!(
            r#"INSERT INTO issue_events (id, issue_id, event_type, actor, payload, occurred_at)
               VALUES ($1, $2, $3, $4, $5, COALESCE($6, datetime('now', 'subsec')))"#,
            id,
            issue_id,
            event_type,
            actor,
            payload,
            occurred_at
        )
        .execute(pool)
        .await?;
        Ok(id)
    }

    /// A ticket's timeline, oldest first.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueEvent,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      event_type as "event_type!: IssueEventType",
                      actor,
                      payload as "payload!: sqlx::types::Json<Value>",
                      occurred_at as "occurred_at!: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_events
               WHERE issue_id = $1
               ORDER BY julianday(occurred_at) ASC, rowid ASC"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueEvent,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      event_type as "event_type!: IssueEventType",
                      actor,
                      payload as "payload!: sqlx::types::Json<Value>",
                      occurred_at as "occurred_at!: DateTime<Utc>",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_events
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// Whether the ticket already has an event of this type whose payload
    /// field `key` equals `value` (prior commits are recorded once).
    pub async fn exists_with(
        pool: &SqlitePool,
        issue_id: Uuid,
        event_type: IssueEventType,
        key: &str,
        value: &str,
    ) -> Result<bool, sqlx::Error> {
        let path = format!("$.{key}");
        let found = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!: i64" FROM issue_events
               WHERE issue_id = $1 AND event_type = $2 AND json_extract(payload, $3) = $4"#,
            issue_id,
            event_type,
            path,
            value
        )
        .fetch_one(pool)
        .await?;
        Ok(found > 0)
    }
}
