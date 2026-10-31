//! Problems a tester run reported in `issues.json` (Phase 6, D23). Open ones
//! go to coder fix runs; a re-test that no longer reports one marks it
//! fixed. You can set one to `wontfix` so the loop leaves it alone.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum IssueSeverity {
    Low,
    Medium,
    High,
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum IssueCategory {
    EdgeCase,
    Integration,
    Logic,
    ErrorHandling,
    StateTiming,
    Requirements,
}

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum IssueFoundStatus {
    Open,
    Fixed,
    Wontfix,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct IssueFound {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub run_id: Option<Uuid>,
    #[ts(type = "number | null")]
    pub checkpoint: Option<i64>,
    pub file: Option<String>,
    #[ts(type = "number | null")]
    pub line: Option<i64>,
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub description: String,
    pub status: IssueFoundStatus,
    #[ts(type = "number | null")]
    pub fixed_round: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One entry of a tester's `issues.json`, read leniently: models write
/// "Edge case", "edge-case" and "EDGE_CASE" for the same thing.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportedIssue {
    /// The id of an earlier issue the tester says is still there.
    pub id: Option<Uuid>,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub description: String,
}

fn norm(s: &str) -> String {
    s.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

impl IssueSeverity {
    /// Unknown severities count as medium.
    pub fn parse_lenient(s: &str) -> Self {
        match norm(s).as_str() {
            "low" | "minor" | "trivial" => Self::Low,
            "high" | "critical" | "major" | "blocker" => Self::High,
            _ => Self::Medium,
        }
    }
}

impl IssueCategory {
    /// Unknown categories count as logic.
    pub fn parse_lenient(s: &str) -> Self {
        match norm(s).as_str() {
            "edge_case" | "edge_cases" | "edgecase" => Self::EdgeCase,
            "integration" => Self::Integration,
            "error_handling" | "errors" | "error" => Self::ErrorHandling,
            "state_timing" | "state" | "timing" | "race" | "concurrency" => Self::StateTiming,
            "requirements" | "requirement" | "spec" => Self::Requirements,
            _ => Self::Logic,
        }
    }
}

/// Why an `issues.json` could not be read.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IssuesJsonError {
    #[error("issues.json is not JSON: {0}")]
    NotJson(String),
    #[error("issues.json must be a list of issues (or {{\"issues\": [...]}})")]
    NotAList,
    #[error("issue {0} has no description")]
    NoDescription(usize),
}

/// Parses `issues.json`: a list, or an object with an `issues` list. An
/// empty list means the checkpoint passed.
pub fn parse_issues_json(text: &str) -> Result<Vec<ReportedIssue>, IssuesJsonError> {
    let value: Value =
        serde_json::from_str(text.trim()).map_err(|e| IssuesJsonError::NotJson(e.to_string()))?;
    let list = match &value {
        Value::Array(list) => list,
        Value::Object(o) => match o.get("issues") {
            Some(Value::Array(list)) => list,
            _ => return Err(IssuesJsonError::NotAList),
        },
        _ => return Err(IssuesJsonError::NotAList),
    };
    let text_of = |v: &Value, key: &str| {
        v.get(key)
            .and_then(|x| match x {
                Value::String(s) => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
    };
    list.iter()
        .enumerate()
        .map(|(i, v)| {
            let description = text_of(v, "description")
                .or_else(|| text_of(v, "title"))
                .ok_or(IssuesJsonError::NoDescription(i + 1))?;
            let line = match v.get("line") {
                Some(Value::Number(n)) => n.as_i64(),
                Some(Value::String(s)) => s.trim().parse().ok(),
                _ => None,
            }
            .filter(|l| *l > 0);
            Ok(ReportedIssue {
                id: text_of(v, "id").and_then(|s| Uuid::parse_str(&s).ok()),
                file: text_of(v, "file"),
                line,
                severity: IssueSeverity::parse_lenient(&text_of(v, "severity").unwrap_or_default()),
                category: IssueCategory::parse_lenient(&text_of(v, "category").unwrap_or_default()),
                description,
            })
        })
        .collect()
}

/// What a tester run changed about a checkpoint's issues.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TestResults {
    pub new: usize,
    pub still_open: usize,
    pub fixed: usize,
}

impl IssueFound {
    pub async fn find_by_issue(
        pool: &SqlitePool,
        issue_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueFound,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      run_id as "run_id: Uuid",
                      checkpoint,
                      file,
                      line,
                      severity as "severity!: IssueSeverity",
                      category as "category!: IssueCategory",
                      description,
                      status as "status!: IssueFoundStatus",
                      fixed_round,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues_found
               WHERE issue_id = $1
               ORDER BY created_at, rowid"#,
            issue_id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            IssueFound,
            r#"SELECT id as "id!: Uuid",
                      issue_id as "issue_id!: Uuid",
                      run_id as "run_id: Uuid",
                      checkpoint,
                      file,
                      line,
                      severity as "severity!: IssueSeverity",
                      category as "category!: IssueCategory",
                      description,
                      status as "status!: IssueFoundStatus",
                      fixed_round,
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM issues_found
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    /// The open issues of one checkpoint, oldest first.
    pub async fn open_at(
        pool: &SqlitePool,
        issue_id: Uuid,
        checkpoint: i64,
    ) -> Result<Vec<Self>, sqlx::Error> {
        Ok(Self::find_by_issue(pool, issue_id)
            .await?
            .into_iter()
            .filter(|i| i.checkpoint == Some(checkpoint) && i.status == IssueFoundStatus::Open)
            .collect())
    }

    /// Stores a tester run's report for a checkpoint. Earlier open issues it
    /// repeats by id stay open; the ones it no longer reports are fixed in
    /// `round`; everything else is new. Repeating a `wontfix` or a fixed one
    /// by id leaves it as it is.
    pub async fn record_test_results(
        pool: &SqlitePool,
        issue_id: Uuid,
        run_id: Option<Uuid>,
        checkpoint: i64,
        round: i64,
        reported: &[ReportedIssue],
    ) -> Result<TestResults, sqlx::Error> {
        // IMMEDIATE: this reads then writes, and a deferred transaction
        // can't upgrade its read lock while another connection writes; that
        // fails with "database is locked" at once, past the busy timeout.
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let known: Vec<(Uuid, IssueFoundStatus)> = sqlx::query_as!(
            KnownRow,
            r#"SELECT id as "id!: Uuid", status as "status!: IssueFoundStatus"
               FROM issues_found WHERE issue_id = $1 AND checkpoint = $2"#,
            issue_id,
            checkpoint
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|r| (r.id, r.status))
        .collect();

        let mut results = TestResults::default();
        let mut repeated = HashSet::new();
        for r in reported {
            if let Some(id) = r.id
                && let Some((_, status)) = known.iter().find(|(k, _)| *k == id)
            {
                if *status == IssueFoundStatus::Open && repeated.insert(id) {
                    results.still_open += 1;
                }
                continue;
            }
            let id = Uuid::new_v4();
            sqlx::query!(
                r#"INSERT INTO issues_found (id, issue_id, run_id, checkpoint, file, line,
                                            severity, category, description)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
                id,
                issue_id,
                run_id,
                checkpoint,
                r.file,
                r.line,
                r.severity,
                r.category,
                r.description
            )
            .execute(&mut *tx)
            .await?;
            results.new += 1;
        }
        for (id, status) in &known {
            if *status == IssueFoundStatus::Open && !repeated.contains(id) {
                sqlx::query!(
                    r#"UPDATE issues_found SET status = 'fixed', fixed_round = $2,
                           updated_at = datetime('now', 'subsec')
                       WHERE id = $1"#,
                    id,
                    round
                )
                .execute(&mut *tx)
                .await?;
                results.fixed += 1;
            }
        }
        tx.commit().await?;
        Ok(results)
    }

    pub async fn set_status(
        pool: &SqlitePool,
        id: Uuid,
        status: IssueFoundStatus,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE issues_found SET status = $2,
                   fixed_round = CASE WHEN $2 = 'fixed' THEN fixed_round ELSE NULL END,
                   updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            status
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// A newly approved plan starts over: the old plan's issues go.
    pub async fn delete_by_issue(pool: &SqlitePool, issue_id: Uuid) -> Result<u64, sqlx::Error> {
        Ok(
            sqlx::query!("DELETE FROM issues_found WHERE issue_id = $1", issue_id)
                .execute(pool)
                .await?
                .rows_affected(),
        )
    }
}

struct KnownRow {
    id: Uuid,
    status: IssueFoundStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_json_is_read_leniently() {
        let id = Uuid::new_v4();
        let issues = parse_issues_json(&format!(
            r#"{{"issues": [
                {{"file": "src/a.rs", "line": "12", "severity": "HIGH",
                  "category": "Edge case", "description": " Empty input panics "}},
                {{"id": "{id}", "category": "state/timing", "title": "Race", "line": 0}}
            ]}}"#
        ))
        .unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].file.as_deref(), Some("src/a.rs"));
        assert_eq!(issues[0].line, Some(12));
        assert_eq!(issues[0].severity, IssueSeverity::High);
        assert_eq!(issues[0].category, IssueCategory::EdgeCase);
        assert_eq!(issues[0].description, "Empty input panics");
        assert_eq!(issues[1].id, Some(id));
        assert_eq!(issues[1].line, None);
        assert_eq!(issues[1].severity, IssueSeverity::Medium);
        assert_eq!(issues[1].category, IssueCategory::StateTiming);
        assert_eq!(issues[1].description, "Race");

        assert_eq!(parse_issues_json("[]").unwrap(), vec![]);
        assert_eq!(
            parse_issues_json("{\"ok\": true}"),
            Err(IssuesJsonError::NotAList)
        );
        assert_eq!(
            parse_issues_json("[{\"file\": \"x\"}]"),
            Err(IssuesJsonError::NoDescription(1))
        );
        assert!(matches!(
            parse_issues_json("nope"),
            Err(IssuesJsonError::NotJson(_))
        ));
    }

    #[test]
    fn categories_cover_the_d23_list() {
        for (text, category) in [
            ("edge case", IssueCategory::EdgeCase),
            ("integration", IssueCategory::Integration),
            ("logic", IssueCategory::Logic),
            ("error handling", IssueCategory::ErrorHandling),
            ("state/timing", IssueCategory::StateTiming),
            ("requirements", IssueCategory::Requirements),
            ("whatever", IssueCategory::Logic),
        ] {
            assert_eq!(IssueCategory::parse_lenient(text), category, "{text}");
        }
    }
}
