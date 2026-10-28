use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use strum_macros::{Display, EnumString};
use ts_rs::TS;
use uuid::Uuid;

use super::issue::IssuePriority;

#[derive(
    Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS, EnumString, Display,
)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum IntakeStatus {
    /// The intake agent is working on it.
    Enhancing,
    /// Waiting for the user's review.
    Ready,
    /// The agent failed; the original can still be reviewed and accepted.
    Failed,
    Accepted,
    Discarded,
}

/// The structured ticket the intake agent writes (`enhanced_ticket.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct EnhancedTicket {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub priority: Option<IssuePriority>,
}

impl EnhancedTicket {
    /// The ticket description: the clean description followed by the
    /// acceptance criteria and open questions as Markdown sections.
    pub fn to_markdown(&self) -> String {
        let mut out = self.description.trim().to_string();
        let mut section = |heading: &str, items: &[String]| {
            let items: Vec<&str> = items
                .iter()
                .map(|i| i.trim())
                .filter(|i| !i.is_empty())
                .collect();
            if items.is_empty() {
                return;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("## {heading}\n"));
            for item in items {
                out.push_str(&format!("\n- {item}"));
            }
        };
        section("Acceptance criteria", &self.acceptance_criteria);
        section("Open questions", &self.open_questions);
        out
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct TicketIntake {
    pub id: Uuid,
    pub project_id: Uuid,
    pub raw_text: String,
    pub status: IntakeStatus,
    pub job_id: Option<Uuid>,
    #[ts(type = "EnhancedTicket | null")]
    pub enhanced: Option<sqlx::types::Json<EnhancedTicket>>,
    pub detected_external_id: Option<String>,
    pub error: Option<String>,
    pub issue_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TicketIntake {
    pub async fn create(
        pool: &SqlitePool,
        project_id: Uuid,
        raw_text: &str,
        attachment_ids: &[Uuid],
        detected_external_id: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        let id = Uuid::new_v4();
        let mut tx = pool.begin().await?;
        sqlx::query!(
            r#"INSERT INTO ticket_intakes (id, project_id, raw_text, detected_external_id)
               VALUES ($1, $2, $3, $4)"#,
            id,
            project_id,
            raw_text,
            detected_external_id
        )
        .execute(&mut *tx)
        .await?;
        for attachment_id in attachment_ids {
            sqlx::query!(
                r#"INSERT OR IGNORE INTO intake_attachments (intake_id, attachment_id)
                   VALUES ($1, $2)"#,
                id,
                attachment_id
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Self::find_by_id(pool, id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            TicketIntake,
            r#"SELECT id as "id!: Uuid",
                      project_id as "project_id!: Uuid",
                      raw_text,
                      status as "status!: IntakeStatus",
                      job_id as "job_id: Uuid",
                      enhanced as "enhanced: sqlx::types::Json<EnhancedTicket>",
                      detected_external_id,
                      error,
                      issue_id as "issue_id: Uuid",
                      created_at as "created_at!: DateTime<Utc>",
                      updated_at as "updated_at!: DateTime<Utc>"
               FROM ticket_intakes
               WHERE id = $1"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    pub async fn attachment_ids(pool: &SqlitePool, id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT attachment_id as "attachment_id!: Uuid"
               FROM intake_attachments WHERE intake_id = $1"#,
            id
        )
        .fetch_all(pool)
        .await
    }

    pub async fn set_job(pool: &SqlitePool, id: Uuid, job_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE ticket_intakes SET job_id = $2, updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            job_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Stores the agent's result (or its failure). Ignored once the intake
    /// was accepted or discarded.
    pub async fn set_result(
        pool: &SqlitePool,
        id: Uuid,
        enhanced: Option<&EnhancedTicket>,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let status = if enhanced.is_some() {
            IntakeStatus::Ready
        } else {
            IntakeStatus::Failed
        };
        let enhanced = enhanced.map(sqlx::types::Json);
        sqlx::query!(
            r#"UPDATE ticket_intakes
               SET status = $2, enhanced = $3, error = $4, updated_at = datetime('now', 'subsec')
               WHERE id = $1 AND status = 'enhancing'"#,
            id,
            status,
            enhanced,
            error
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn resolve(
        pool: &SqlitePool,
        id: Uuid,
        status: IntakeStatus,
        issue_id: Option<Uuid>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"UPDATE ticket_intakes
               SET status = $2, issue_id = $3, updated_at = datetime('now', 'subsec')
               WHERE id = $1"#,
            id,
            status,
            issue_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_sections() {
        let ticket = EnhancedTicket {
            title: "T".into(),
            description: "Body.\n".into(),
            acceptance_criteria: vec!["One".into(), " ".into(), "Two".into()],
            open_questions: vec![],
            ..Default::default()
        };
        assert_eq!(
            ticket.to_markdown(),
            "Body.\n\n## Acceptance criteria\n\n- One\n- Two"
        );
        let empty = EnhancedTicket {
            open_questions: vec!["Why?".into()],
            ..Default::default()
        };
        assert_eq!(empty.to_markdown(), "## Open questions\n\n- Why?");
    }
}
