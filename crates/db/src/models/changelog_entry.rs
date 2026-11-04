use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type, types::Json};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

/// What a changelog entry is about.
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ChangelogKind {
    /// A subtask's coder run, merged into the ticket branch.
    Coder,
    /// A tester run at a checkpoint.
    Tester,
    /// A fix run on the tester's open issues.
    Fix,
    /// The ticket merged into its target branch.
    Merge,
}

/// A commit an entry refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CommitRef {
    pub repo: String,
    pub sha: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ChangelogEntry {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub run_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub kind: ChangelogKind,
    pub title: String,
    pub summary: String,
    /// `repo/path` per changed file.
    #[ts(type = "string[]")]
    pub files: Json<Vec<String>>,
    #[ts(type = "CommitRef[]")]
    pub commit_refs: Json<Vec<CommitRef>>,
    /// Issues found this run fixed (tester: confirmed by the re-test; fix
    /// run: the open issues it was sent to fix).
    #[ts(type = "string[]")]
    pub issues_fixed: Json<Vec<Uuid>>,
    /// Issues a tester run found.
    #[ts(type = "string[]")]
    pub issues_found: Json<Vec<Uuid>>,
    pub scribe_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct CreateChangelogEntry<'a> {
    pub issue_id: Uuid,
    pub run_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub kind: ChangelogKind,
    pub title: &'a str,
    pub summary: &'a str,
    pub files: &'a [String],
    pub commit_refs: &'a [CommitRef],
    pub issues_fixed: &'a [Uuid],
    pub issues_found: &'a [Uuid],
    pub scribe_error: Option<&'a str>,
}

impl ChangelogEntry {
    /// Stores an entry. A run that already has an entry of this kind keeps
    /// the first one and gets `None`: end-of-run handling may repeat.
    pub async fn create(
        pool: &SqlitePool,
        data: CreateChangelogEntry<'_>,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"INSERT INTO changelog_entries
                   (id, issue_id, run_id, job_id, kind, title, summary, files, commit_refs,
                    issues_fixed, issues_found, scribe_error)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
               ON CONFLICT DO NOTHING
               RETURNING *"#,
        )
        .bind(Uuid::new_v4())
        .bind(data.issue_id)
        .bind(data.run_id)
        .bind(data.job_id)
        .bind(data.kind)
        .bind(data.title)
        .bind(data.summary)
        .bind(Json(data.files))
        .bind(Json(data.commit_refs))
        .bind(Json(data.issues_fixed))
        .bind(Json(data.issues_found))
        .bind(data.scribe_error)
        .fetch_optional(pool)
        .await
    }

    /// A ticket's entries, oldest first.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            "SELECT * FROM changelog_entries WHERE issue_id = $1 ORDER BY created_at, rowid",
        )
        .bind(issue_id)
        .fetch_all(pool)
        .await
    }

    pub async fn exists_for_run(
        pool: &SqlitePool,
        run_id: Uuid,
        kind: ChangelogKind,
    ) -> Result<bool, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM changelog_entries WHERE run_id = $1 AND kind = $2",
        )
        .bind(run_id)
        .bind(kind)
        .fetch_one(pool)
        .await?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            r#"CREATE TABLE changelog_entries (
                id BLOB PRIMARY KEY, issue_id BLOB NOT NULL, run_id BLOB, job_id BLOB,
                kind TEXT NOT NULL, title TEXT NOT NULL, summary TEXT NOT NULL,
                files TEXT NOT NULL DEFAULT '[]', commit_refs TEXT NOT NULL DEFAULT '[]',
                issues_fixed TEXT NOT NULL DEFAULT '[]', issues_found TEXT NOT NULL DEFAULT '[]',
                scribe_error TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')));
               CREATE UNIQUE INDEX r ON changelog_entries (run_id, kind)
                   WHERE run_id IS NOT NULL;"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn entry<'a>(
        issue_id: Uuid,
        run_id: Option<Uuid>,
        refs: &'a [CommitRef],
    ) -> CreateChangelogEntry<'a> {
        CreateChangelogEntry {
            issue_id,
            run_id,
            job_id: None,
            kind: ChangelogKind::Coder,
            title: "Add login",
            summary: "## What changed\nLogin.",
            files: &[],
            commit_refs: refs,
            issues_fixed: &[],
            issues_found: &[],
            scribe_error: None,
        }
    }

    #[tokio::test]
    async fn one_entry_per_run_and_kind() {
        let pool = pool().await;
        let issue = Uuid::new_v4();
        let run = Uuid::new_v4();
        let refs = [CommitRef {
            repo: "app".into(),
            sha: "abc".into(),
        }];
        let first = ChangelogEntry::create(&pool, entry(issue, Some(run), &refs))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.commit_refs.0, refs);
        assert!(
            ChangelogEntry::create(&pool, entry(issue, Some(run), &refs))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ChangelogEntry::exists_for_run(&pool, run, ChangelogKind::Coder)
                .await
                .unwrap()
        );
        // Merge entries have no run and may repeat (merge, reopen, merge).
        ChangelogEntry::create(&pool, entry(issue, None, &[]))
            .await
            .unwrap()
            .unwrap();
        ChangelogEntry::create(&pool, entry(issue, None, &[]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ChangelogEntry::find_by_issue(&pool, issue)
                .await
                .unwrap()
                .len(),
            3
        );
    }
}
