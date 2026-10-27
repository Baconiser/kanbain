use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum IssueRelationshipType {
    Blocking,
    Related,
    HasDuplicate,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueRelationship {
    pub id: Uuid,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub related_issue_id: Uuid,
    pub relationship_type: IssueRelationshipType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateIssueRelationship {
    pub related_issue_id: Uuid,
    pub relationship_type: IssueRelationshipType,
}

impl IssueRelationship {
    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueRelationship,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      related_issue_id as "related_issue_id!: Uuid",
                      relationship_type as "relationship_type!: IssueRelationshipType",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_relationships
               WHERE project_id = $1
               ORDER BY created_at ASC"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueRelationship,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      related_issue_id as "related_issue_id!: Uuid",
                      relationship_type as "relationship_type!: IssueRelationshipType",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_relationships
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueRelationship,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      related_issue_id as "related_issue_id!: Uuid",
                      relationship_type as "relationship_type!: IssueRelationshipType",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_relationships
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// Creates a relationship. Returns the existing row if it already exists.
    pub async fn create(
        pool: &SqlitePool,
        project_id: Uuid,
        issue_id: Uuid,
        data: &CreateIssueRelationship,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO issue_relationships (id, project_id, issue_id, related_issue_id, relationship_type)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (issue_id, related_issue_id, relationship_type) DO NOTHING"#,
            id,
            project_id,
            issue_id,
            data.related_issue_id,
            data.relationship_type
        )
        .execute(pool)
        .await?;
        sqlx::query_as!(
            IssueRelationship,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      related_issue_id as "related_issue_id!: Uuid",
                      relationship_type as "relationship_type!: IssueRelationshipType",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_relationships
               WHERE issue_id = $1 AND related_issue_id = $2 AND relationship_type = $3"#,
            issue_id,
            data.related_issue_id,
            data.relationship_type
        )
        .fetch_one(pool)
        .await
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!("DELETE FROM issue_relationships WHERE id = $1", id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}
