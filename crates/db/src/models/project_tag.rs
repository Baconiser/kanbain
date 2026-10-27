use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;

/// A ticket label within a project. Not to be confused with `tag::Tag`,
/// which holds reusable prompt snippets.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ProjectTag {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateProjectTag {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct UpdateProjectTag {
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub color: Option<String>,
}

impl ProjectTag {
    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid", name, color
               FROM project_tags
               WHERE project_id = $1
               ORDER BY name COLLATE NOCASE ASC"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid", name, color
               FROM project_tags
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid", name, color
               FROM project_tags
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn create(
        pool: &SqlitePool,
        project_id: Uuid,
        data: &CreateProjectTag,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let name = data.name.trim();
        sqlx::query_as!(
            ProjectTag,
            r#"INSERT INTO project_tags (id, project_id, name, color)
               VALUES ($1, $2, $3, $4)
               RETURNING id as "id!: Uuid", project_id as "project_id!: Uuid", name, color"#,
            id,
            project_id,
            name,
            data.color
        )
        .fetch_one(pool)
        .await
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateProjectTag,
    ) -> Result<Self, sqlx::Error> {
        let existing = Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;
        let name = data
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&existing.name);
        let color = data.color.as_ref().unwrap_or(&existing.color);
        sqlx::query_as!(
            ProjectTag,
            r#"UPDATE project_tags SET name = $2, color = $3
               WHERE id = $1
               RETURNING id as "id!: Uuid", project_id as "project_id!: Uuid", name, color"#,
            id,
            name,
            color
        )
        .fetch_one(pool)
        .await
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!("DELETE FROM project_tags WHERE id = $1", id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueTag {
    pub id: Uuid,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub tag_id: Uuid,
}

impl IssueTag {
    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid", tag_id as "tag_id!: Uuid"
               FROM issue_tags
               WHERE project_id = $1"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid", tag_id as "tag_id!: Uuid"
               FROM issue_tags
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// Adds a tag to an issue. Returns the existing row if already tagged.
    pub async fn add(
        pool: &SqlitePool,
        project_id: Uuid,
        issue_id: Uuid,
        tag_id: Uuid,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO issue_tags (id, project_id, issue_id, tag_id)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (issue_id, tag_id) DO NOTHING"#,
            id,
            project_id,
            issue_id,
            tag_id
        )
        .execute(pool)
        .await?;
        sqlx::query_as!(
            IssueTag,
            r#"SELECT id as "id!: Uuid", project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid", tag_id as "tag_id!: Uuid"
               FROM issue_tags
               WHERE issue_id = $1 AND tag_id = $2"#,
            issue_id,
            tag_id
        )
        .fetch_one(pool)
        .await
    }

    pub async fn remove(
        pool: &SqlitePool,
        issue_id: Uuid,
        tag_id: Uuid,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            "DELETE FROM issue_tags WHERE issue_id = $1 AND tag_id = $2",
            issue_id,
            tag_id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
