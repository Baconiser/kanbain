use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;

/// Author value for comments written by the user.
pub const USER_AUTHOR: &str = "user";

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueComment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub parent_id: Option<Uuid>,
    /// `user`, or `agent:<run id>` for comments posted by an agent run.
    pub author: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateIssueComment {
    pub message: String,
    #[serde(default)]
    #[ts(optional)]
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct UpdateIssueComment {
    pub message: String,
}

impl IssueComment {
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueComment,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      parent_id as "parent_id: Uuid",
                      author,
                      message,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issue_comments
               WHERE issue_id = $1
               ORDER BY created_at ASC"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueComment,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      parent_id as "parent_id: Uuid",
                      author,
                      message,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issue_comments
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueComment,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      parent_id as "parent_id: Uuid",
                      author,
                      message,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issue_comments
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn create(
        pool: &SqlitePool,
        issue_id: Uuid,
        author: &str,
        data: &CreateIssueComment,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query_as!(
            IssueComment,
            r#"INSERT INTO issue_comments (id, issue_id, parent_id, author, message)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id as "id!: Uuid",
                         issue_id as "issue_id!: Uuid",
                         parent_id as "parent_id: Uuid",
                         author,
                         message,
                         created_at as "created_at!: DateTime<Utc>",
                         updated_at as "updated_at!: DateTime<Utc>""#,
            id,
            issue_id,
            data.parent_id,
            author,
            data.message
        )
        .fetch_one(pool)
        .await
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateIssueComment,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as!(
            IssueComment,
            r#"UPDATE issue_comments
               SET message = $2, updated_at = datetime('now', 'subsec')
               WHERE id = $1
               RETURNING id as "id!: Uuid",
                         issue_id as "issue_id!: Uuid",
                         parent_id as "parent_id: Uuid",
                         author,
                         message,
                         created_at as "created_at!: DateTime<Utc>",
                         updated_at as "updated_at!: DateTime<Utc>""#,
            id,
            data.message
        )
        .fetch_one(pool)
        .await
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!("DELETE FROM issue_comments WHERE id = $1", id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}
