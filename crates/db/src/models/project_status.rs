use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// The pipeline role of a status column. The pipeline refers to statuses by
/// `kind`, never by name, so users can rename columns freely.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ProjectStatusKind {
    Backlog,
    Planning,
    AwaitingApproval,
    InProgress,
    Testing,
    Review,
    Done,
    Blocked,
    Custom,
}

/// Default statuses for a new project: (name, HSL color, kind, hidden).
pub const DEFAULT_STATUSES: &[(&str, &str, ProjectStatusKind, bool)] = &[
    ("Backlog", "220 9% 46%", ProjectStatusKind::Backlog, false),
    (
        "Planning",
        "199 89% 48%",
        ProjectStatusKind::Planning,
        false,
    ),
    (
        "Awaiting approval",
        "45 93% 47%",
        ProjectStatusKind::AwaitingApproval,
        false,
    ),
    (
        "In progress",
        "38 92% 50%",
        ProjectStatusKind::InProgress,
        false,
    ),
    ("Testing", "173 80% 40%", ProjectStatusKind::Testing, false),
    ("Review", "258 90% 66%", ProjectStatusKind::Review, false),
    ("Done", "142 71% 45%", ProjectStatusKind::Done, false),
    ("Blocked", "0 84% 60%", ProjectStatusKind::Blocked, false),
];

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ProjectStatus {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
    #[ts(type = "number")]
    pub sort_order: i64,
    pub hidden: bool,
    pub kind: ProjectStatusKind,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateProjectStatus {
    pub name: String,
    pub color: String,
    #[serde(default)]
    #[ts(optional)]
    pub kind: Option<ProjectStatusKind>,
    #[serde(default)]
    #[ts(optional)]
    pub hidden: Option<bool>,
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct UpdateProjectStatus {
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub color: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub kind: Option<ProjectStatusKind>,
    #[serde(default)]
    #[ts(optional)]
    pub hidden: Option<bool>,
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub sort_order: Option<i64>,
}

impl ProjectStatus {
    pub async fn seed_defaults(
        conn: &mut SqliteConnection,
        project_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        for (sort_order, (name, color, kind, hidden)) in DEFAULT_STATUSES.iter().enumerate() {
            let id = Uuid::new_v4();
            let sort_order = sort_order as i64;
            sqlx::query!(
                r#"INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden, kind)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
                id,
                project_id,
                name,
                color,
                sort_order,
                hidden,
                kind
            )
            .execute(&mut *conn)
            .await?;
        }
        Ok(())
    }

    pub async fn find_by_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectStatus,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      name,
                      color,
                      sort_order,
                      hidden as "hidden!: bool",
                      kind as "kind!: ProjectStatusKind",
                      created_at as "created_at!: DateTime<Utc>"
               FROM project_statuses
               WHERE project_id = $1
               ORDER BY sort_order ASC, created_at ASC"#,
            project_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectStatus,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      name,
                      color,
                      sort_order,
                      hidden as "hidden!: bool",
                      kind as "kind!: ProjectStatusKind",
                      created_at as "created_at!: DateTime<Utc>"
               FROM project_statuses
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ProjectStatus,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      name,
                      color,
                      sort_order,
                      hidden as "hidden!: bool",
                      kind as "kind!: ProjectStatusKind",
                      created_at as "created_at!: DateTime<Utc>"
               FROM project_statuses
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// The first status of the given kind, by column order.
    pub async fn find_first_of_kind(
        pool: &SqlitePool,
        project_id: Uuid,
        kind: ProjectStatusKind,
    ) -> Result<Option<Self>, sqlx::Error> {
        Ok(Self::find_by_project(pool, project_id)
            .await?
            .into_iter()
            .find(|s| s.kind == kind))
    }

    pub async fn create(
        pool: &SqlitePool,
        project_id: Uuid,
        data: &CreateProjectStatus,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let kind = data.kind.unwrap_or(ProjectStatusKind::Custom);
        let hidden = data.hidden.unwrap_or(false);
        let name = data.name.trim();
        sqlx::query!(
            r#"INSERT INTO project_statuses (id, project_id, name, color, sort_order, hidden, kind)
               VALUES ($1, $2, $3, $4,
                       COALESCE($5, (SELECT COALESCE(MAX(sort_order), -1) + 1
                                     FROM project_statuses WHERE project_id = $2)),
                       $6, $7)"#,
            id,
            project_id,
            name,
            data.color,
            data.sort_order,
            hidden,
            kind
        )
        .execute(pool)
        .await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateProjectStatus,
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
        let kind = data.kind.unwrap_or(existing.kind);
        let hidden = data.hidden.unwrap_or(existing.hidden);
        let sort_order = data.sort_order.unwrap_or(existing.sort_order);
        sqlx::query!(
            r#"UPDATE project_statuses
               SET name = $2, color = $3, kind = $4, hidden = $5, sort_order = $6
               WHERE id = $1"#,
            id,
            name,
            color,
            kind,
            hidden,
            sort_order
        )
        .execute(pool)
        .await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Deletes a status, moving its issues to `move_to` first.
    pub async fn delete(pool: &SqlitePool, id: Uuid, move_to: Uuid) -> Result<u64, sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query!(
            r#"UPDATE issues SET status_id = $2, updated_at = datetime('now', 'subsec')
               WHERE status_id = $1"#,
            id,
            move_to
        )
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query!("DELETE FROM project_statuses WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}
