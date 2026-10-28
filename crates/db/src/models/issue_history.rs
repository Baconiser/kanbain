use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

use super::issue_event::sqlite_time;

/// The user's answer to "Found N commits for X. Allow analysis?" (D18).
#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum HistoryPermission {
    Pending,
    Allowed,
    Denied,
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum HistoryStatus {
    Idle,
    Analyzing,
    Failed,
}

/// A ticket's git-history check and prior-work summary.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueHistory {
    pub issue_id: Uuid,
    pub searched_id: String,
    #[ts(type = "number")]
    pub commit_count: i64,
    /// Commits the history agent may analyze: not made by KanbAIn's agents.
    #[ts(type = "number")]
    pub prior_count: i64,
    pub prior_hash: String,
    pub permission: HistoryPermission,
    pub status: HistoryStatus,
    pub prior_work: Option<String>,
    pub analyzed_hash: Option<String>,
    pub job_id: Option<Uuid>,
    pub error: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub analyzed_at: Option<DateTime<Utc>>,
}

impl IssueHistory {
    /// The prior-work summary is missing or older than the commits.
    pub fn needs_analysis(&self) -> bool {
        self.prior_count > 0 && self.analyzed_hash.as_deref() != Some(self.prior_hash.as_str())
    }
}

/// A commit found for a ticket.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct TicketCommit {
    pub issue_id: Uuid,
    pub repo_id: Uuid,
    pub repo_name: String,
    pub sha: String,
    pub author: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
    #[ts(type = "string[]")]
    pub branches: sqlx::types::Json<Vec<String>>,
    pub message_match: bool,
}

/// A file a ticket's commits changed (E19), summed over the commits.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct TicketFileLink {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub path: String,
    /// `''` until code is chunked (Phase 3d).
    pub symbol: String,
    #[ts(type = "number")]
    pub commits: i64,
    #[ts(type = "number")]
    pub additions: i64,
    #[ts(type = "number")]
    pub deletions: i64,
    pub last_commit_sha: String,
}

/// A commit to store, with the files it changed.
#[derive(Debug, Clone)]
pub struct NewTicketCommit {
    pub sha: String,
    pub author: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
    pub branches: Vec<String>,
    pub message_match: bool,
    /// (path, additions, deletions); counts are `None` for binary files.
    pub files: Vec<(String, Option<i64>, Option<i64>)>,
}

impl IssueHistory {
    pub async fn find(pool: &SqlitePool, issue_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueHistory,
            r#"SELECT issue_id as "issue_id!: Uuid",
                      searched_id,
                      commit_count,
                      prior_count,
                      prior_hash,
                      permission as "permission!: HistoryPermission",
                      status as "status!: HistoryStatus",
                      prior_work,
                      analyzed_hash,
                      job_id as "job_id: Uuid",
                      error,
                      checked_at as "checked_at!: DateTime<Utc>",
                      analyzed_at as "analyzed_at: DateTime<Utc>"
               FROM issue_history
               WHERE issue_id = $1"#,
            issue_id
        )
        .fetch_optional(pool)
        .await
    }

    /// Records a check's result, keeping the permission and summary.
    pub async fn record_check(
        pool: &SqlitePool,
        issue_id: Uuid,
        searched_id: &str,
        commit_count: i64,
        prior_count: i64,
        prior_hash: &str,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query!(
            r#"INSERT INTO issue_history (issue_id, searched_id, commit_count, prior_count,
                                          prior_hash)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (issue_id) DO UPDATE
                   SET searched_id = excluded.searched_id,
                       commit_count = excluded.commit_count,
                       prior_count = excluded.prior_count,
                       prior_hash = excluded.prior_hash,
                       checked_at = datetime('now', 'subsec')"#,
            issue_id,
            searched_id,
            commit_count,
            prior_count,
            prior_hash
        )
        .execute(pool)
        .await?;
        Self::find(pool, issue_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn set_permission(
        pool: &SqlitePool,
        issue_id: Uuid,
        permission: HistoryPermission,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE issue_history SET permission = $2 WHERE issue_id = $1",
            issue_id,
            permission
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Marks the ticket as being analyzed. False when it already is.
    pub async fn start_analysis(pool: &SqlitePool, issue_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE issue_history SET status = 'analyzing', error = NULL
               WHERE issue_id = $1 AND status != 'analyzing'"#,
            issue_id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn finish_analysis(
        pool: &SqlitePool,
        issue_id: Uuid,
        job_id: Option<Uuid>,
        result: Result<(&str, &str), &str>,
    ) -> Result<(), sqlx::Error> {
        match result {
            Ok((prior_work, analyzed_hash)) => {
                sqlx::query!(
                    r#"UPDATE issue_history
                       SET status = 'idle', prior_work = $2, analyzed_hash = $3, job_id = $4,
                           error = NULL, analyzed_at = datetime('now', 'subsec')
                       WHERE issue_id = $1"#,
                    issue_id,
                    prior_work,
                    analyzed_hash,
                    job_id
                )
                .execute(pool)
                .await?;
            }
            Err(error) => {
                sqlx::query!(
                    r#"UPDATE issue_history SET status = 'failed', error = $2, job_id = $3
                       WHERE issue_id = $1"#,
                    issue_id,
                    error,
                    job_id
                )
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Analyses left running by a previous launch can never finish.
    pub async fn fail_interrupted(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE issue_history SET status = 'failed',
                   error = 'Interrupted by an app restart'
               WHERE status = 'analyzing'"#
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}

impl TicketCommit {
    /// Replaces a ticket's commits and file links in one repo.
    pub async fn replace_for_repo(
        pool: &SqlitePool,
        issue_id: Uuid,
        repo_id: Uuid,
        commits: &[NewTicketCommit],
    ) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query!(
            "DELETE FROM ticket_commits WHERE issue_id = $1 AND repo_id = $2",
            issue_id,
            repo_id
        )
        .execute(&mut *tx)
        .await?;
        // Symbol links (Phase 3d) are rebuilt from the file links.
        sqlx::query!(
            "DELETE FROM ticket_code_links WHERE issue_id = $1 AND repo_id = $2",
            issue_id,
            repo_id
        )
        .execute(&mut *tx)
        .await?;
        for commit in commits {
            let committed_at = sqlite_time(commit.committed_at);
            let branches = sqlx::types::Json(&commit.branches);
            sqlx::query!(
                r#"INSERT INTO ticket_commits (issue_id, repo_id, sha, author, committed_at,
                                               subject, branches, message_match)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                issue_id,
                repo_id,
                commit.sha,
                commit.author,
                committed_at,
                commit.subject,
                branches,
                commit.message_match
            )
            .execute(&mut *tx)
            .await?;
            for (path, additions, deletions) in &commit.files {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO ticket_code_links
                           (issue_id, repo_id, path, commit_sha, additions, deletions)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                    issue_id,
                    repo_id,
                    path,
                    commit.sha,
                    additions,
                    deletions
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await
    }

    /// A ticket's commits, newest first.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            TicketCommit,
            r#"SELECT c.issue_id as "issue_id!: Uuid",
                      c.repo_id as "repo_id!: Uuid",
                      r.name as "repo_name!",
                      c.sha,
                      c.author,
                      c.committed_at as "committed_at!: DateTime<Utc>",
                      c.subject,
                      c.branches as "branches!: sqlx::types::Json<Vec<String>>",
                      c.message_match as "message_match!: bool"
               FROM ticket_commits c
               JOIN repos r ON r.id = c.repo_id
               WHERE c.issue_id = $1
               ORDER BY julianday(c.committed_at) DESC"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }
}

impl TicketFileLink {
    /// The files a ticket's commits changed, most changed first.
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            TicketFileLink,
            r#"SELECT l.repo_id as "repo_id!: Uuid",
                      r.name as "repo_name!",
                      l.path,
                      l.symbol,
                      COUNT(DISTINCT l.commit_sha) as "commits!: i64",
                      COALESCE(SUM(l.additions), 0) as "additions!: i64",
                      COALESCE(SUM(l.deletions), 0) as "deletions!: i64",
                      (SELECT c.sha FROM ticket_commits c
                       JOIN ticket_code_links l2 ON l2.commit_sha = c.sha
                            AND l2.issue_id = c.issue_id AND l2.repo_id = c.repo_id
                       WHERE c.issue_id = l.issue_id AND c.repo_id = l.repo_id
                             AND l2.path = l.path
                       ORDER BY julianday(c.committed_at) DESC LIMIT 1) as "last_commit_sha!: String"
               FROM ticket_code_links l
               JOIN repos r ON r.id = l.repo_id
               WHERE l.issue_id = $1
               GROUP BY l.repo_id, l.path, l.symbol
               ORDER BY COUNT(DISTINCT l.commit_sha) DESC,
                        COALESCE(SUM(l.additions), 0) + COALESCE(SUM(l.deletions), 0) DESC,
                        l.path ASC"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }
}
