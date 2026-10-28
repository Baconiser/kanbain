//! SQLite index of the agent library files (see the `agent_library`
//! service). Rows are written only by the sync, never by API handlers.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct AgentDefinitionRow {
    pub name: String,
    pub definition: Option<String>,
    pub error: Option<String>,
    pub content_hash: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct StyleBlockRow {
    pub name: String,
    pub body: String,
    pub content_hash: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct SkillRow {
    pub name: String,
    pub description: Option<String>,
    pub file_count: i64,
    pub total_bytes: i64,
    pub error: Option<String>,
    pub content_hash: String,
    pub updated_at: DateTime<Utc>,
}

impl AgentDefinitionRow {
    pub async fn list(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            AgentDefinitionRow,
            r#"SELECT name as "name!", definition, error, content_hash,
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM agent_definitions ORDER BY name"#
        )
        .fetch_all(pool)
        .await
    }

    /// Insert or update; leaves the row (and `updated_at`) alone when the
    /// file hasn't changed.
    pub async fn upsert(
        pool: &SqlitePool,
        name: &str,
        definition: Option<&str>,
        error: Option<&str>,
        content_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO agent_definitions (name, definition, error, content_hash)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT(name) DO UPDATE SET
                   definition = excluded.definition,
                   error = excluded.error,
                   content_hash = excluded.content_hash,
                   updated_at = datetime('now', 'subsec')
               WHERE agent_definitions.content_hash != excluded.content_hash"#,
            name,
            definition,
            error,
            content_hash
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete_except(pool: &SqlitePool, keep: &[String]) -> Result<(), sqlx::Error> {
        let keep = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
        sqlx::query!(
            "DELETE FROM agent_definitions WHERE name NOT IN (SELECT value FROM json_each($1))",
            keep
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl StyleBlockRow {
    pub async fn list(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            StyleBlockRow,
            r#"SELECT name as "name!", body, content_hash,
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM style_blocks ORDER BY name"#
        )
        .fetch_all(pool)
        .await
    }

    pub async fn upsert(
        pool: &SqlitePool,
        name: &str,
        body: &str,
        content_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO style_blocks (name, body, content_hash)
               VALUES ($1, $2, $3)
               ON CONFLICT(name) DO UPDATE SET
                   body = excluded.body,
                   content_hash = excluded.content_hash,
                   updated_at = datetime('now', 'subsec')
               WHERE style_blocks.content_hash != excluded.content_hash"#,
            name,
            body,
            content_hash
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete_except(pool: &SqlitePool, keep: &[String]) -> Result<(), sqlx::Error> {
        let keep = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
        sqlx::query!(
            "DELETE FROM style_blocks WHERE name NOT IN (SELECT value FROM json_each($1))",
            keep
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

impl SkillRow {
    pub async fn list(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            SkillRow,
            r#"SELECT name as "name!", description, file_count, total_bytes, error,
                      content_hash, updated_at as "updated_at!: DateTime<Utc>"
               FROM skills ORDER BY name"#
        )
        .fetch_all(pool)
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        pool: &SqlitePool,
        name: &str,
        description: Option<&str>,
        file_count: i64,
        total_bytes: i64,
        error: Option<&str>,
        content_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO skills (name, description, file_count, total_bytes, error, content_hash)
               VALUES ($1, $2, $3, $4, $5, $6)
               ON CONFLICT(name) DO UPDATE SET
                   description = excluded.description,
                   file_count = excluded.file_count,
                   total_bytes = excluded.total_bytes,
                   error = excluded.error,
                   content_hash = excluded.content_hash,
                   updated_at = datetime('now', 'subsec')
               WHERE skills.content_hash != excluded.content_hash"#,
            name,
            description,
            file_count,
            total_bytes,
            error,
            content_hash
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete_except(pool: &SqlitePool, keep: &[String]) -> Result<(), sqlx::Error> {
        let keep = serde_json::to_string(keep).unwrap_or_else(|_| "[]".into());
        sqlx::query!(
            "DELETE FROM skills WHERE name NOT IN (SELECT value FROM json_each($1))",
            keep
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}
