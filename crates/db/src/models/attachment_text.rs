use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;

/// Text extracted from an attachment (Phase 3a), for the intake agent and
/// for search indexing.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct AttachmentText {
    pub attachment_id: Uuid,
    /// `pdf`, `html`, `docx`, `xml` or `text`.
    pub format: String,
    pub text: String,
    /// Why extraction failed, when it did.
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl AttachmentText {
    pub async fn upsert(
        pool: &SqlitePool,
        attachment_id: Uuid,
        format: &str,
        text: &str,
        error: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as!(
            AttachmentText,
            r#"INSERT INTO attachment_texts (attachment_id, format, text, error)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (attachment_id) DO UPDATE
                   SET format = excluded.format, text = excluded.text, error = excluded.error,
                       created_at = datetime('now', 'subsec')
               RETURNING attachment_id as "attachment_id!: Uuid",
                         format,
                         text,
                         error,
                         created_at as "created_at!: DateTime<Utc>""#,
            attachment_id,
            format,
            text,
            error
        )
        .fetch_one(pool)
        .await
    }

    pub async fn find(pool: &SqlitePool, attachment_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            AttachmentText,
            r#"SELECT attachment_id as "attachment_id!: Uuid",
                      format,
                      text,
                      error,
                      created_at as "created_at!: DateTime<Utc>"
               FROM attachment_texts
               WHERE attachment_id = $1"#,
            attachment_id
        )
        .fetch_optional(pool)
        .await
    }

    /// Texts of a ticket's attachments.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            AttachmentText,
            r#"SELECT t.attachment_id as "attachment_id!: Uuid",
                      t.format,
                      t.text,
                      t.error,
                      t.created_at as "created_at!: DateTime<Utc>"
               FROM attachment_texts t
               JOIN issue_attachments ia ON ia.attachment_id = t.attachment_id
               WHERE ia.issue_id = $1"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }
}
