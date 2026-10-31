use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::rust::double_option;
use sqlx::{FromRow, SqlitePool, types::Json};
use ts_rs::TS;
use uuid::Uuid;

use super::{project_status::ProjectStatus, repo::Repo};

pub const DEFAULT_ID_PREFIX: &str = "KAN";

/// Per-project settings stored as JSON. New fields need a serde default so
/// older rows keep deserializing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct ProjectSettings {
    /// Skip the "allow history analysis?" prompt for this project's tickets.
    #[serde(default)]
    pub history_always_allow: bool,
    /// Extra hosts sandbox containers may reach, on top of the default
    /// package registries (`*.example.com` matches subdomains).
    #[serde(default)]
    pub sandbox_allowed_domains: Vec<String>,
    /// Path to a Dockerfile (starting `FROM kanbain-agent`) used instead of
    /// the base image for this project's runs.
    #[serde(default)]
    pub sandbox_dockerfile: Option<String>,
    /// Let code go to a code embedder that is not on this machine or its
    /// private network (E9). Off by default.
    #[serde(default)]
    pub allow_remote_code_embedding: bool,
    /// Globs of files the code index skips, e.g. generated code (E14).
    #[serde(default)]
    pub search_exclude_globs: Vec<String>,
    /// Globs (relative to each repo) of the files the tester may add or
    /// change (D23). Empty means [`DEFAULT_TEST_PATHS`].
    #[serde(default)]
    pub test_paths: Vec<String>,
}

/// Where tests live in the common languages: test directories, then the
/// usual test file names.
pub const DEFAULT_TEST_PATHS: &[&str] = &[
    "**/tests/**",
    "**/test/**",
    "**/__tests__/**",
    "**/spec/**",
    "**/e2e/**",
    "**/*.test.*",
    "**/*.spec.*",
    "**/*_test.*",
    "**/*_spec.*",
    "**/test_*.py",
    "**/*Test.java",
    "**/*Tests.java",
    "**/*Test.kt",
    "**/*Tests.cs",
    "**/*Test.php",
];

impl ProjectSettings {
    /// The tester's test paths: the project's, or the defaults.
    pub fn effective_test_paths(&self) -> Vec<String> {
        let own: Vec<String> = self
            .test_paths
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if own.is_empty() {
            DEFAULT_TEST_PATHS.iter().map(|p| p.to_string()).collect()
        } else {
            own
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub id_prefix: String,
    pub external_id_pattern: Option<String>,
    pub external_id_key: Option<String>,
    #[ts(type = "number")]
    pub sort_order: i64,
    #[ts(type = "ProjectSettings")]
    pub settings: Json<ProjectSettings>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
pub struct CreateProject {
    pub name: String,
    #[serde(default)]
    pub id_prefix: Option<String>,
    #[serde(default)]
    pub repo_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct UpdateProject {
    #[serde(default)]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub id_prefix: Option<String>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "string | null")]
    pub external_id_pattern: Option<Option<String>>,
    #[serde(default, with = "double_option")]
    #[ts(optional, type = "string | null")]
    pub external_id_key: Option<Option<String>>,
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub sort_order: Option<i64>,
    #[serde(default)]
    #[ts(optional)]
    pub settings: Option<ProjectSettings>,
}

/// Normalizes a user-supplied ticket prefix: uppercase ASCII alphanumerics,
/// starting with a letter, at most 10 characters.
pub fn normalize_id_prefix(raw: &str) -> Option<String> {
    let prefix: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .take(10)
        .collect();
    match prefix.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => Some(prefix),
        _ => None,
    }
}

impl Project {
    pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            Project,
            r#"SELECT id as "id!: Uuid",
                      name,
                      id_prefix,
                      external_id_pattern,
                      external_id_key,
                      sort_order,
                      settings as "settings!: Json<ProjectSettings>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM projects
               ORDER BY sort_order ASC, created_at ASC"#
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Project,
            r#"SELECT id as "id!: Uuid",
                      name,
                      id_prefix,
                      external_id_pattern,
                      external_id_key,
                      sort_order,
                      settings as "settings!: Json<ProjectSettings>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM projects
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            Project,
            r#"SELECT id as "id!: Uuid",
                      name,
                      id_prefix,
                      external_id_pattern,
                      external_id_key,
                      sort_order,
                      settings as "settings!: Json<ProjectSettings>",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM projects
               WHERE rowid = $1"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// Creates a project, seeds the default statuses and links the given repos.
    pub async fn create(pool: &SqlitePool, data: &CreateProject) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let id_prefix = data
            .id_prefix
            .as_deref()
            .and_then(normalize_id_prefix)
            .unwrap_or_else(|| DEFAULT_ID_PREFIX.to_string());
        let name = data.name.trim();

        let mut tx = pool.begin().await?;
        sqlx::query!(
            r#"INSERT INTO projects (id, name, id_prefix, sort_order)
               VALUES ($1, $2, $3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects))"#,
            id,
            name,
            id_prefix
        )
        .execute(&mut *tx)
        .await?;
        ProjectStatus::seed_defaults(&mut tx, id).await?;
        for repo_id in &data.repo_ids {
            ProjectRepo::link(&mut *tx, id, *repo_id).await?;
        }
        tx.commit().await?;

        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateProject,
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
        let id_prefix = data
            .id_prefix
            .as_deref()
            .and_then(normalize_id_prefix)
            .unwrap_or(existing.id_prefix.clone());
        let external_id_pattern = data
            .external_id_pattern
            .clone()
            .unwrap_or(existing.external_id_pattern.clone())
            .filter(|p| !p.trim().is_empty());
        let external_id_key = data
            .external_id_key
            .clone()
            .unwrap_or(existing.external_id_key.clone())
            .filter(|k| !k.trim().is_empty());
        let sort_order = data.sort_order.unwrap_or(existing.sort_order);
        let settings = Json(data.settings.clone().unwrap_or(existing.settings.0));

        sqlx::query!(
            r#"UPDATE projects
               SET name = $2, id_prefix = $3, external_id_pattern = $4,
                   external_id_key = $5, sort_order = $6, settings = $7,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            name,
            id_prefix,
            external_id_pattern,
            external_id_key,
            sort_order,
            settings
        )
        .execute(pool)
        .await?;

        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<u64, sqlx::Error> {
        // Issues reference statuses without a cascade, so remove them first.
        let mut tx = pool.begin().await?;
        sqlx::query!("DELETE FROM issues WHERE project_id = $1", id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query!("DELETE FROM projects WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ProjectRepo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub repo_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl ProjectRepo {
    pub async fn link<'e, E>(
        executor: E,
        project_id: Uuid,
        repo_id: Uuid,
    ) -> Result<(), sqlx::Error>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO project_repos (id, project_id, repo_id)
               VALUES ($1, $2, $3)
               ON CONFLICT (project_id, repo_id) DO NOTHING"#,
            id,
            project_id,
            repo_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    pub async fn unlink(
        pool: &SqlitePool,
        project_id: Uuid,
        repo_id: Uuid,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            "DELETE FROM project_repos WHERE project_id = $1 AND repo_id = $2",
            project_id,
            repo_id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn find_repos_for_project(
        pool: &SqlitePool,
        project_id: Uuid,
    ) -> Result<Vec<Repo>, sqlx::Error> {
        let repo_ids = sqlx::query_scalar!(
            r#"SELECT repo_id as "repo_id!: Uuid"
               FROM project_repos
               WHERE project_id = $1
               ORDER BY created_at ASC"#,
            project_id
        )
        .fetch_all(pool)
        .await?;

        let mut repos = Vec::with_capacity(repo_ids.len());
        for repo_id in repo_ids {
            if let Some(repo) = Repo::find_by_id(pool, repo_id).await? {
                repos.push(repo);
            }
        }
        Ok(repos)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_id_prefix;

    #[test]
    fn id_prefix_is_normalized() {
        assert_eq!(normalize_id_prefix(" kan "), Some("KAN".to_string()));
        assert_eq!(normalize_id_prefix("my-proj"), Some("MYPROJ".to_string()));
        assert_eq!(normalize_id_prefix("1abc"), None);
        assert_eq!(normalize_id_prefix(""), None);
        assert_eq!(
            normalize_id_prefix("ABCDEFGHIJKLMNOP"),
            Some("ABCDEFGHIJ".to_string())
        );
    }
}
