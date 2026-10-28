//! Ticket intake (D15–D17): a dropped ticket (text plus attachments) is
//! enhanced by the intake agent, reviewed by the user, then becomes a new
//! ticket or a new description version of the ticket with the same
//! External ID.

use std::str::FromStr;

use db::{
    DBService,
    models::{
        agent_job::{AgentJob, AgentJobStatus, JOB_KIND_INTAKE},
        attachment_text::AttachmentText,
        file::{File, IssueAttachment},
        issue::{CreateIssue, Issue, IssueMode, IssuePriority, UpdateIssue},
        issue_description::{DescriptionKind, NewDescriptionVersion},
        issue_event::{IssueEvent, IssueEventType},
        project::Project,
        ticket_intake::{EnhancedTicket, IntakeStatus, TicketIntake},
    },
};
use executors::adapters::{AgentRole, headless::extract_json_object};
use serde::Deserialize;
use serde_json::{Value, json};
use services::services::file::FileService;
use ts_rs::TS;
use utils::ticket_id::detect_external_id;
use uuid::Uuid;

use crate::jobs::{AgentJobs, JobRequest};

/// Largest task message sent to the intake agent, in bytes.
const MAX_TASK_BYTES: usize = 120 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Issue(#[from] db::models::issue::IssueError),
    #[error("{0}")]
    BadRequest(String),
    #[error("Intake not found")]
    NotFound,
}

/// The reviewed ticket, as the user accepts it.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct AcceptIntake {
    pub title: String,
    pub description: String,
    #[serde(default)]
    #[ts(optional)]
    pub external_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub priority: Option<IssuePriority>,
    #[serde(default)]
    #[ts(optional)]
    pub mode: Option<IssueMode>,
    #[serde(default)]
    #[ts(optional)]
    pub status_id: Option<Uuid>,
    /// Add a new description version to this ticket instead of creating one
    /// (D17).
    #[serde(default)]
    #[ts(optional)]
    pub update_issue_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct IntakeService {
    db: DBService,
    files: FileService,
    jobs: AgentJobs,
}

/// The External ID for a project, detected in `text`.
pub fn detect_for_project(project: &Project, text: &str) -> Option<String> {
    detect_external_id(
        text,
        project.external_id_pattern.as_deref(),
        project.external_id_key.as_deref(),
        Some(&project.id_prefix),
    )
}

/// The intake agent's task: the rules, the expected JSON and the dropped
/// text with its attachments' text, capped at [`MAX_TASK_BYTES`].
pub fn intake_task(
    project_name: &str,
    raw_text: &str,
    attachments: &[(String, String)],
    detected_external_id: Option<&str>,
) -> String {
    let mut task = format!(
        "A ticket was dropped into the project \"{project_name}\". Turn it into a structured \
         ticket.\n\n\
         Answer with one JSON object and nothing else:\n\
         {{\"title\": string, \"description\": string, \"acceptance_criteria\": [string], \
         \"open_questions\": [string], \"external_id\": string or null, \
         \"priority\": \"urgent\" | \"high\" | \"medium\" | \"low\" | null}}\n\n\
         Rules:\n\
         - Keep the original meaning. Do not invent requirements.\n\
         - `title`: one short line, without the ticket key.\n\
         - `description`: clean Markdown (convert Jira wiki markup), without the acceptance \
         criteria and open questions, which go in their own fields.\n\
         - `acceptance_criteria`: testable statements from the ticket; empty if it has none.\n\
         - `open_questions`: what is unclear or missing.\n\
         - `external_id`: the source ticket key (for example PROJ-123), or null.\n\
         - `priority`: your guess from the text, or null.\n"
    );
    if let Some(id) = detected_external_id {
        task.push_str(&format!("\nKanbAIn detected the key {id} in the text.\n"));
    }
    task.push_str("\n<ticket>\n");
    task.push_str(raw_text.trim());
    task.push_str("\n</ticket>\n");
    for (name, text) in attachments {
        if text.trim().is_empty() {
            continue;
        }
        task.push_str(&format!(
            "\n<attachment name=\"{name}\">\n{}\n</attachment>\n",
            text.trim()
        ));
    }
    if task.len() > MAX_TASK_BYTES {
        let mut cut = MAX_TASK_BYTES;
        while !task.is_char_boundary(cut) {
            cut -= 1;
        }
        task.truncate(cut);
        task.push_str("\n[… the rest was cut to fit …]\n");
    }
    task
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => vec![],
    }
}

/// The agent's answer as an [`EnhancedTicket`], leniently: unknown
/// priorities are dropped, and an External ID that is not in the source
/// text is replaced by the detected one.
pub fn parse_enhanced(
    answer: &str,
    source: &str,
    detected: Option<&str>,
) -> Option<EnhancedTicket> {
    let value = extract_json_object(answer)?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let title = text("title")?;
    let external_id = text("external_id")
        .map(|id| id.trim_matches(|c| c == '[' || c == ']').to_string())
        .filter(|id| source.to_lowercase().contains(&id.to_lowercase()))
        .or_else(|| detected.map(str::to_string));
    Some(EnhancedTicket {
        title,
        description: text("description").unwrap_or_default(),
        acceptance_criteria: string_list(value.get("acceptance_criteria")),
        open_questions: string_list(value.get("open_questions")),
        external_id,
        priority: text("priority").and_then(|p| IssuePriority::from_str(&p.to_lowercase()).ok()),
    })
}

/// Which description version an accepted intake becomes.
pub fn accepted_kind(
    raw_text: &str,
    enhanced: Option<&EnhancedTicket>,
    title: &str,
    description: &str,
) -> DescriptionKind {
    if let Some(e) = enhanced
        && e.title.trim() == title.trim()
        && e.to_markdown().trim() == description.trim()
    {
        return DescriptionKind::Enhanced;
    }
    if description.trim() == raw_text.trim() {
        DescriptionKind::Original
    } else {
        DescriptionKind::Edited
    }
}

impl IntakeService {
    pub fn new(db: DBService, files: FileService, jobs: AgentJobs) -> Self {
        Self { db, files, jobs }
    }

    /// Extracted texts of the intake's attachments, as (name, text).
    async fn attachment_texts(
        &self,
        intake_id: Uuid,
    ) -> Result<Vec<(String, String)>, IntakeError> {
        let pool = &self.db.pool;
        let mut texts = Vec::new();
        for id in TicketIntake::attachment_ids(pool, intake_id).await? {
            if let Some(file) = File::find_by_id(pool, id).await?
                && let Some(text) = AttachmentText::find(pool, id).await?
                && text.error.is_none()
            {
                texts.push((file.original_name, text.text));
            }
        }
        Ok(texts)
    }

    /// Stores the dropped ticket, extracts its attachments' text and starts
    /// the intake agent in the background.
    pub async fn start(
        &self,
        project: &Project,
        raw_text: &str,
        attachment_ids: &[Uuid],
    ) -> Result<TicketIntake, IntakeError> {
        let pool = &self.db.pool;
        let mut all_text = raw_text.to_string();
        for id in attachment_ids {
            let file = File::find_by_id(pool, *id)
                .await?
                .ok_or_else(|| IntakeError::BadRequest(format!("Attachment {id} not found")))?;
            match self.files.ensure_text(&file).await {
                Ok(Some(text)) => {
                    all_text.push('\n');
                    all_text.push_str(&text.text);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("Extracting {} failed: {e}", file.original_name),
            }
        }
        if all_text.trim().is_empty() {
            return Err(IntakeError::BadRequest(
                "Paste the ticket text or drop a file with text".to_string(),
            ));
        }
        let detected = detect_for_project(project, &all_text);
        let intake = TicketIntake::create(
            pool,
            project.id,
            raw_text,
            attachment_ids,
            detected.as_deref(),
        )
        .await?;
        self.spawn_agent(project.clone(), intake.id);
        Ok(intake)
    }

    /// Runs the intake agent again.
    pub async fn retry(&self, intake_id: Uuid) -> Result<TicketIntake, IntakeError> {
        let pool = &self.db.pool;
        let intake = TicketIntake::find_by_id(pool, intake_id)
            .await?
            .ok_or(IntakeError::NotFound)?;
        if matches!(
            intake.status,
            IntakeStatus::Accepted | IntakeStatus::Discarded
        ) {
            return Err(IntakeError::BadRequest("This intake is closed".to_string()));
        }
        let project = Project::find_by_id(pool, intake.project_id)
            .await?
            .ok_or(IntakeError::NotFound)?;
        sqlx::query("UPDATE ticket_intakes SET status = 'enhancing', error = NULL WHERE id = $1")
            .bind(intake_id)
            .execute(pool)
            .await?;
        self.spawn_agent(project, intake_id);
        TicketIntake::find_by_id(pool, intake_id)
            .await?
            .ok_or(IntakeError::NotFound)
    }

    fn spawn_agent(&self, project: Project, intake_id: Uuid) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.enhance(project, intake_id).await {
                tracing::error!("Intake {intake_id} failed: {e}");
                let _ = TicketIntake::set_result(
                    &service.db.pool,
                    intake_id,
                    None,
                    Some(&e.to_string()),
                )
                .await;
            }
        });
    }

    async fn enhance(&self, project: Project, intake_id: Uuid) -> Result<(), IntakeError> {
        let pool = &self.db.pool;
        let intake = TicketIntake::find_by_id(pool, intake_id)
            .await?
            .ok_or(IntakeError::NotFound)?;
        let attachments = self.attachment_texts(intake_id).await?;
        let task = intake_task(
            &project.name,
            &intake.raw_text,
            &attachments,
            intake.detected_external_id.as_deref(),
        );
        let mut context = vec![("ticket_raw.md".to_string(), intake.raw_text.clone())];
        for (name, text) in &attachments {
            context.push((format!("attachments/{name}.txt"), text.clone()));
        }
        let (job_id, result) = self
            .jobs
            .run(JobRequest {
                kind: JOB_KIND_INTAKE,
                role: AgentRole::Intake,
                project: Some(project),
                issue_id: None,
                context,
                task,
            })
            .await;
        if let Some(job_id) = job_id {
            TicketIntake::set_job(pool, intake_id, job_id).await?;
        }
        let source = std::iter::once(intake.raw_text.as_str())
            .chain(attachments.iter().map(|(_, t)| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        match result {
            Ok(answer) => {
                match parse_enhanced(&answer, &source, intake.detected_external_id.as_deref()) {
                    Some(enhanced) => {
                        TicketIntake::set_result(pool, intake_id, Some(&enhanced), None).await?
                    }
                    None => {
                        TicketIntake::set_result(
                            pool,
                            intake_id,
                            None,
                            Some("The intake agent did not answer with a ticket"),
                        )
                        .await?
                    }
                }
            }
            Err(e) => TicketIntake::set_result(pool, intake_id, None, Some(&e)).await?,
        }
        Ok(())
    }

    /// Creates the ticket, or adds a description version to an existing
    /// one, and links the dropped attachments.
    pub async fn accept(&self, intake_id: Uuid, data: &AcceptIntake) -> Result<Issue, IntakeError> {
        let pool = &self.db.pool;
        let intake = TicketIntake::find_by_id(pool, intake_id)
            .await?
            .ok_or(IntakeError::NotFound)?;
        if matches!(
            intake.status,
            IntakeStatus::Accepted | IntakeStatus::Discarded
        ) {
            return Err(IntakeError::BadRequest("This intake is closed".to_string()));
        }
        if data.title.trim().is_empty() {
            return Err(IntakeError::BadRequest("A title is required".to_string()));
        }
        let enhanced = intake.enhanced.as_ref().map(|e| &e.0);
        let version = NewDescriptionVersion {
            kind: accepted_kind(&intake.raw_text, enhanced, &data.title, &data.description),
            original_text: Some(intake.raw_text.clone()),
            enhanced_text: enhanced.map(|e| e.to_markdown()),
            intake_id: Some(intake_id),
        };
        let description = Some(data.description.clone()).filter(|d| !d.trim().is_empty());

        let (issue, action) = match data.update_issue_id {
            Some(issue_id) => {
                let existing = Issue::find_by_id(pool, issue_id)
                    .await?
                    .ok_or(IntakeError::NotFound)?;
                if existing.project_id != intake.project_id {
                    return Err(IntakeError::BadRequest(
                        "That ticket is in another project".to_string(),
                    ));
                }
                let update = UpdateIssue {
                    title: Some(data.title.clone()),
                    description: Some(description),
                    external_id: data.external_id.clone().map(Some),
                    priority: data.priority.map(Some),
                    ..Default::default()
                };
                let issue =
                    Issue::update_with_version(pool, issue_id, &update, Some(&version)).await?;
                (issue, "updated")
            }
            None => {
                let create = CreateIssue {
                    title: data.title.clone(),
                    description,
                    status_id: data.status_id,
                    priority: data.priority,
                    sort_order: None,
                    parent_issue_id: None,
                    parent_issue_sort_order: None,
                    external_id: data.external_id.clone(),
                    mode: data.mode,
                    budget_usd: None,
                };
                let issue =
                    Issue::create_with_version(pool, intake.project_id, &create, &version).await?;
                (issue, "created")
            }
        };

        let attachment_ids = TicketIntake::attachment_ids(pool, intake_id).await?;
        IssueAttachment::associate_many(pool, issue.id, &attachment_ids).await?;

        let job = match intake.job_id {
            Some(id) => AgentJob::find_by_id(pool, id).await?,
            None => None,
        };
        let actor = match &job {
            Some(job) if job.status == AgentJobStatus::Completed => format!("agent:{}", job.id),
            _ => "user".to_string(),
        };
        let payload = json!({
            "intake_id": intake_id,
            "action": action,
            "enhanced": enhanced.is_some(),
            "kind": version.kind,
            "job_id": job.as_ref().map(|j| j.id),
            "agent": job.as_ref().and_then(|j| j.agent_def_id.clone()),
            "model": job.as_ref().and_then(|j| j.model.clone()),
            "tokens_in": job.as_ref().map(|j| j.tokens_in),
            "tokens_out": job.as_ref().map(|j| j.tokens_out),
            "cost_usd": job.as_ref().map(|j| j.cost_usd),
            "attachments": attachment_ids.len(),
        });
        IssueEvent::create(
            pool,
            issue.id,
            IssueEventType::Intake,
            &actor,
            &payload,
            None,
        )
        .await?;
        if let Some(job) = &job
            && job.issue_id.is_none()
        {
            sqlx::query("UPDATE agent_jobs SET issue_id = $2 WHERE id = $1")
                .bind(job.id)
                .bind(issue.id)
                .execute(pool)
                .await?;
        }
        TicketIntake::resolve(pool, intake_id, IntakeStatus::Accepted, Some(issue.id)).await?;
        Ok(issue)
    }

    pub async fn discard(&self, intake_id: Uuid) -> Result<(), IntakeError> {
        TicketIntake::resolve(&self.db.pool, intake_id, IntakeStatus::Discarded, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_includes_ticket_and_attachments() {
        let task = intake_task(
            "Shop",
            "Login broken [PROJ-1]",
            &[
                ("spec.pdf".into(), "Steps: open page".into()),
                ("x.png".into(), "".into()),
            ],
            Some("PROJ-1"),
        );
        assert!(task.contains("<ticket>\nLogin broken [PROJ-1]\n</ticket>"));
        assert!(task.contains("<attachment name=\"spec.pdf\">\nSteps: open page"));
        assert!(!task.contains("x.png"));
        assert!(task.contains("detected the key PROJ-1"));
        let long = intake_task("P", &"x".repeat(MAX_TASK_BYTES * 2), &[], None);
        assert!(long.len() < MAX_TASK_BYTES + 100);
    }

    #[test]
    fn parses_lenient_answers() {
        let answer = r#"```json
{"title": "Reset mail missing", "description": "Mail never arrives.",
 "acceptance_criteria": ["Mail arrives within 1 minute", ""], "open_questions": "Which provider?",
 "external_id": "[PROJ-9]", "priority": "HIGH"}
```"#;
        let t = parse_enhanced(answer, "proj-9 mail", None).unwrap();
        assert_eq!(t.title, "Reset mail missing");
        assert_eq!(t.acceptance_criteria, vec!["Mail arrives within 1 minute"]);
        assert_eq!(t.open_questions, vec!["Which provider?"]);
        assert_eq!(t.external_id.as_deref(), Some("PROJ-9"));
        assert_eq!(t.priority, Some(IssuePriority::High));

        // An id that is not in the source is replaced by the detected one.
        let invented = r#"{"title": "T", "external_id": "FAKE-1", "priority": "soon"}"#;
        let t = parse_enhanced(invented, "text with PROJ-2", Some("PROJ-2")).unwrap();
        assert_eq!(t.external_id.as_deref(), Some("PROJ-2"));
        assert_eq!(t.priority, None);

        assert!(parse_enhanced("{\"description\": \"no title\"}", "", None).is_none());
        assert!(parse_enhanced("I can't", "", None).is_none());
    }

    #[test]
    fn accepted_kinds() {
        let enhanced = EnhancedTicket {
            title: "T".into(),
            description: "D".into(),
            acceptance_criteria: vec!["A".into()],
            ..Default::default()
        };
        let md = enhanced.to_markdown();
        assert_eq!(
            accepted_kind("raw", Some(&enhanced), "T", &md),
            DescriptionKind::Enhanced
        );
        assert_eq!(
            accepted_kind("raw", Some(&enhanced), "T2", &md),
            DescriptionKind::Edited
        );
        assert_eq!(
            accepted_kind("raw", None, "T", " raw "),
            DescriptionKind::Original
        );
        assert_eq!(
            accepted_kind("raw", None, "T", "other"),
            DescriptionKind::Edited
        );
    }
}
