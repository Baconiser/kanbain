use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// Where a description version's text came from (D17).
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum DescriptionKind {
    /// Typed or dropped text, stored as is.
    Original,
    /// The intake agent's version, accepted unchanged.
    Enhanced,
    /// Changed by the user.
    Edited,
}

/// One version of a ticket's title and description. The raw dropped text and
/// the intake agent's version are kept next to the stored description.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueDescription {
    pub id: Uuid,
    pub issue_id: Uuid,
    #[ts(type = "number")]
    pub version: i64,
    pub kind: DescriptionKind,
    pub title: String,
    pub description: Option<String>,
    pub original_text: Option<String>,
    pub enhanced_text: Option<String>,
    pub intake_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// Facts about a new version beyond its title and description.
#[derive(Debug, Clone)]
pub struct NewDescriptionVersion {
    pub kind: DescriptionKind,
    pub original_text: Option<String>,
    pub enhanced_text: Option<String>,
    pub intake_id: Option<Uuid>,
}

impl NewDescriptionVersion {
    pub fn plain(kind: DescriptionKind) -> Self {
        Self {
            kind,
            original_text: None,
            enhanced_text: None,
            intake_id: None,
        }
    }
}

impl IssueDescription {
    /// Adds the next version inside the caller's transaction.
    pub async fn insert_next(
        conn: &mut SqliteConnection,
        issue_id: Uuid,
        title: &str,
        description: Option<&str>,
        meta: &NewDescriptionVersion,
    ) -> Result<i64, sqlx::Error> {
        let id = Uuid::new_v4();
        sqlx::query_scalar!(
            r#"INSERT INTO issue_descriptions
                   (id, issue_id, version, kind, title, description, original_text,
                    enhanced_text, intake_id)
               VALUES ($1, $2,
                       (SELECT COALESCE(MAX(version), 0) + 1 FROM issue_descriptions
                        WHERE issue_id = $2),
                       $3, $4, $5, $6, $7, $8)
               RETURNING version as "version!: i64""#,
            id,
            issue_id,
            meta.kind,
            title,
            description,
            meta.original_text,
            meta.enhanced_text,
            meta.intake_id
        )
        .fetch_one(conn)
        .await
    }

    /// Records a user edit. Edits within ten minutes of the previous edit
    /// version update it instead of adding one, so the description's
    /// autosave doesn't produce a version per pause in typing.
    pub async fn record_edit(
        conn: &mut SqliteConnection,
        issue_id: Uuid,
        title: &str,
        description: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let recent = sqlx::query_scalar!(
            r#"SELECT id as "id!: Uuid" FROM issue_descriptions
               WHERE issue_id = $1
                 AND version = (SELECT MAX(version) FROM issue_descriptions WHERE issue_id = $1)
                 AND kind = 'edited' AND intake_id IS NULL
                 AND julianday(created_at) > julianday('now', '-10 minutes')"#,
            issue_id
        )
        .fetch_optional(&mut *conn)
        .await?;
        match recent {
            Some(id) => {
                sqlx::query_scalar!(
                    r#"UPDATE issue_descriptions SET title = $2, description = $3
                   WHERE id = $1 RETURNING version as "version!: i64""#,
                    id,
                    title,
                    description
                )
                .fetch_one(conn)
                .await
            }
            None => {
                Self::insert_next(
                    conn,
                    issue_id,
                    title,
                    description,
                    &NewDescriptionVersion::plain(DescriptionKind::Edited),
                )
                .await
            }
        }
    }

    /// All versions of a ticket, newest first.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueDescription,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      version,
                      kind as "kind!: DescriptionKind",
                      title,
                      description,
                      original_text,
                      enhanced_text,
                      intake_id as "intake_id: Uuid",
                      created_at as "created_at!: DateTime<Utc>"
               FROM issue_descriptions
               WHERE issue_id = $1
               ORDER BY version DESC"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }
}
