use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// What a run event records.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, Display, EnumString,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum ExecutionProcessEventType {
    /// One model call through the proxy (model, tokens, cost, latency).
    ModelCall,
    ToolCall,
    /// A request from the container refused by the egress proxy.
    EgressBlock,
    /// The run reached its budget and was stopped.
    BudgetExceeded,
    /// Sandbox lifecycle (image build, container start, code transfer).
    Sandbox,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ExecutionProcessEvent {
    pub id: Uuid,
    pub execution_process_id: Uuid,
    pub event_type: ExecutionProcessEventType,
    #[ts(type = "Record<string, unknown>")]
    pub payload: sqlx::types::Json<Value>,
    pub created_at: DateTime<Utc>,
}

impl ExecutionProcessEvent {
    pub async fn create(
        pool: &SqlitePool,
        execution_process_id: Uuid,
        event_type: ExecutionProcessEventType,
        payload: &Value,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let payload = sqlx::types::Json(payload);
        sqlx::query_as!(
            ExecutionProcessEvent,
            r#"INSERT INTO execution_process_events
                   (id, execution_process_id, event_type, payload, created_at)
               VALUES (?, ?, ?, ?, ?)
               RETURNING id as "id!: Uuid",
                         execution_process_id as "execution_process_id!: Uuid",
                         event_type as "event_type!: ExecutionProcessEventType",
                         payload as "payload!: sqlx::types::Json<Value>",
                         created_at as "created_at!: DateTime<Utc>""#,
            id,
            execution_process_id,
            event_type,
            payload,
            now
        )
        .fetch_one(pool)
        .await
    }

    pub async fn find_by_execution_process(
        pool: &SqlitePool,
        execution_process_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcessEvent,
            r#"SELECT id as "id!: Uuid",
                      execution_process_id as "execution_process_id!: Uuid",
                      event_type as "event_type!: ExecutionProcessEventType",
                      payload as "payload!: sqlx::types::Json<Value>",
                      created_at as "created_at!: DateTime<Utc>"
               FROM execution_process_events
               WHERE execution_process_id = ?
               ORDER BY created_at ASC"#,
            execution_process_id
        )
        .fetch_all(pool)
        .await
    }
}
