//! Problems agents notice on the way. A run that spots a bug or wrong code
//! outside its task calls `report_problem` and carries on: the report is
//! handed to the triage agent, a background job whose only task is to check
//! the backlog for a ticket that already covers it, then file a new Backlog
//! ticket, add the finding to the existing one, or drop it.
//!
//! When the triage agent can't run (no sandbox, no agent with the role, no
//! usable answer) the report is still kept: a ticket whose title shares most
//! of its words gets it as a comment, otherwise it becomes a new ticket.

use std::collections::HashSet;

use db::{
    DBService,
    models::{
        agent_job::JOB_KIND_TRIAGE,
        execution_process::ExecutionProcess,
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::{CreateIssue, Issue, IssuePriority},
        issue_comment::{CreateIssueComment, IssueComment},
        issue_relationship::{CreateIssueRelationship, IssueRelationship, IssueRelationshipType},
        project::Project,
        project_status::{ProjectStatus, ProjectStatusKind},
    },
};
use executors::adapters::{AgentRole, headless::extract_json_object};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::jobs::{AgentJobs, JobRequest};

pub const REPORT_TOOL: &str = "report_problem";

/// Problems one run may hand off.
pub const MAX_REPORTS_PER_RUN: usize = 5;
/// Title word overlap (Jaccard) at which the fallback treats a ticket as the
/// same one.
const SAME_TITLE: f64 = 0.6;
const MAX_TITLE: usize = 200;
/// Tickets listed for the triage agent.
const MAX_LISTED_TICKETS: usize = 300;

pub fn definition() -> Value {
    json!({
        "name": REPORT_TOOL,
        "description": "Hand off a bug or wrong code you noticed that is outside your current \
            task, so it is not lost. A triage agent checks the backlog for a ticket that already \
            covers it and files a new Backlog ticket or adds your finding to the existing one. \
            Returns at once; carry on with your task. Do not use it for problems you are fixing \
            now.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short summary of the problem, e.g. \"Ticket counter skips numbers after a failed insert\"."
                },
                "description": {
                    "type": "string",
                    "description": "What is wrong, how you noticed it, and what the correct behavior would be. Markdown."
                },
                "file": {"type": "string", "description": "Repo-relative path where the problem is, if known."},
                "line": {"type": "integer", "minimum": 1, "description": "Line in `file`, if known."},
                "severity": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "urgent"],
                    "description": "How bad it is; becomes the ticket's priority."
                }
            },
            "required": ["title", "description"]
        }
    })
}

/// What a run handed off.
#[derive(Debug, Clone, PartialEq)]
pub struct ProblemReport {
    pub title: String,
    pub description: String,
    /// `path:line` or `path`.
    pub location: Option<String>,
    pub priority: Option<IssuePriority>,
}

fn priority(severity: Option<&str>) -> Option<IssuePriority> {
    match severity?.trim().to_ascii_lowercase().as_str() {
        "urgent" | "critical" | "blocker" => Some(IssuePriority::Urgent),
        "high" | "major" => Some(IssuePriority::High),
        "medium" => Some(IssuePriority::Medium),
        "low" | "minor" | "trivial" => Some(IssuePriority::Low),
        _ => None,
    }
}

fn location(file: Option<&str>, line: Option<u64>) -> Option<String> {
    let file = file.map(str::trim).filter(|f| !f.is_empty())?;
    Some(match line {
        Some(line) => format!("{file}:{line}"),
        None => file.to_string(),
    })
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

pub fn parse_report(arguments: &Value) -> Result<ProblemReport, String> {
    let title = text(arguments, "title").ok_or("`title` is required")?;
    let description = text(arguments, "description").ok_or("`description` is required")?;
    Ok(ProblemReport {
        title: title.chars().take(MAX_TITLE).collect(),
        description: description.to_string(),
        location: location(
            text(arguments, "file"),
            arguments.get("line").and_then(Value::as_u64),
        ),
        priority: priority(text(arguments, "severity")),
    })
}

/// Lowercased words of three or more letters or digits.
fn title_words(title: &str) -> HashSet<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// Share of the words two titles have in common (Jaccard).
pub fn title_overlap(a: &str, b: &str) -> f64 {
    let (a, b) = (title_words(a), title_words(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / a.union(&b).count() as f64
}

/// A new ticket's description: the problem, where, and who found it.
pub fn ticket_description(
    description: &str,
    location: Option<&str>,
    agent: &str,
    source: &str,
) -> String {
    let mut out = description.trim().to_string();
    if let Some(location) = location {
        out.push_str(&format!("\n\n**Where:** `{location}`"));
    }
    out.push_str(&format!(
        "\n\n---\n_Found by the `{agent}` agent while working on {source}._"
    ));
    out
}

/// The comment added to a ticket that already covers a report.
pub fn duplicate_comment(report: &ProblemReport, note: Option<&str>, source: &str) -> String {
    let mut out = format!(
        "An agent ran into this again while working on {source}:\n\n**{}**\n\n{}",
        report.title, report.description
    );
    if let Some(location) = &report.location {
        out.push_str(&format!("\n\n**Where:** `{location}`"));
    }
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(&format!("\n\n**Triage:** {note}"));
    }
    out
}

/// What the triage agent decided.
#[derive(Debug, Clone, PartialEq)]
pub enum TriageDecision {
    /// File a new ticket; title and description as the agent cleaned them.
    Create {
        title: String,
        description: String,
        priority: Option<IssuePriority>,
    },
    /// Add the finding to this existing ticket (its id, e.g. `KAN-12`).
    Duplicate {
        ticket: String,
        note: Option<String>,
    },
    /// Not a real problem, or too vague to act on.
    Drop { reason: String },
}

/// The triage agent's answer, leniently.
pub fn parse_decision(answer: &str) -> Option<TriageDecision> {
    let value = extract_json_object(answer)?;
    match text(&value, "action")?.to_ascii_lowercase().as_str() {
        "create" | "new" => Some(TriageDecision::Create {
            title: text(&value, "title")?.chars().take(MAX_TITLE).collect(),
            description: text(&value, "description")?.to_string(),
            priority: priority(text(&value, "priority")),
        }),
        "duplicate" | "existing" => Some(TriageDecision::Duplicate {
            ticket: text(&value, "ticket")?.trim_start_matches('#').to_string(),
            note: text(&value, "note").map(str::to_string),
        }),
        "drop" => Some(TriageDecision::Drop {
            reason: text(&value, "reason")
                .unwrap_or("no reason given")
                .to_string(),
        }),
        _ => None,
    }
}

/// The triage agent's task: the report, where it came from, and the
/// expected answer.
pub fn triage_task(project: &str, source: &str, agent: &str, report: &ProblemReport) -> String {
    let mut task = format!(
        "The `{agent}` agent found a problem while working on {source} in the project \
         \"{project}\" and handed it to you. Your only job: make sure it ends up in the backlog \
         exactly once.\n\n\
         1. Look for an existing ticket that already covers the same problem. \
         /kanbain/context/tickets.md lists the project's tickets; call search_knowledge with \
         kinds [\"ticket\", \"comment\"] to find similar ones by meaning. A ticket about the same \
         root cause counts, even if worded differently. A Done ticket only counts if the report \
         adds nothing new (otherwise the problem came back: file a new ticket that mentions it).\n\
         2. Answer with one JSON object and nothing else, one of:\n\
         {{\"action\": \"duplicate\", \"ticket\": \"<id, e.g. KAN-12>\", \"note\": \"what the report adds, or null\"}}\n\
         {{\"action\": \"create\", \"title\": string, \"description\": string, \
         \"priority\": \"urgent\" | \"high\" | \"medium\" | \"low\" | null}}\n\
         {{\"action\": \"drop\", \"reason\": string}}\n\n\
         Rules:\n\
         - `create`: a short title and a clean Markdown description of what is wrong, where, \
         and the expected behavior. Keep the reporter's facts; do not invent any.\n\
         - `drop` only when the report is clearly not a problem or too vague to act on.\n\
         - Do not change code.\n\n<report>\nTitle: {}\n",
        report.title
    );
    if let Some(location) = &report.location {
        task.push_str(&format!("Where: {location}\n"));
    }
    if let Some(priority) = report.priority {
        task.push_str(&format!("Severity: {priority}\n"));
    }
    task.push_str(&format!("\n{}\n</report>\n", report.description.trim()));
    task
}

/// The project's tickets for the triage agent, open ones first.
fn ticket_list(tickets: &[Issue], statuses: &[ProjectStatus], prefix: &str) -> String {
    let status = |t: &Issue| statuses.iter().find(|s| s.id == t.status_id);
    let is_done = |t: &Issue| status(t).is_some_and(|s| s.kind == ProjectStatusKind::Done);
    let mut sorted: Vec<&Issue> = tickets.iter().collect();
    sorted.sort_by_key(|t| (is_done(t), std::cmp::Reverse(t.number)));
    let mut out = String::from("# Tickets\n\n");
    for t in sorted.iter().take(MAX_LISTED_TICKETS) {
        let name = status(t).map(|s| s.name.as_str()).unwrap_or("?");
        out.push_str(&format!(
            "- {} [{name}] {}\n",
            t.display_id(prefix),
            t.title
        ));
    }
    if sorted.len() > MAX_LISTED_TICKETS {
        out.push_str(&format!(
            "\n… and {} older tickets; use search_knowledge to find them.\n",
            sorted.len() - MAX_LISTED_TICKETS
        ));
    }
    out
}

/// Where a report came from.
struct Origin {
    run_id: Uuid,
    agent: String,
    source: Issue,
    project: Project,
}

impl Origin {
    fn source_id(&self) -> String {
        self.source.display_id(&self.project.id_prefix)
    }
}

/// Takes reports from runs and has the triage agent file them.
#[derive(Clone)]
pub struct ProblemReports {
    db: DBService,
    jobs: AgentJobs,
}

impl ProblemReports {
    pub fn new(db: DBService, jobs: AgentJobs) -> Self {
        Self { db, jobs }
    }

    async fn log(&self, run_id: Uuid, payload: Value) {
        let _ = ExecutionProcessEvent::create(
            &self.db.pool,
            run_id,
            ExecutionProcessEventType::ToolCall,
            &payload,
        )
        .await;
    }

    /// Reports this run handed off so far.
    async fn handed_off(&self, run_id: Uuid) -> Result<usize, sqlx::Error> {
        Ok(
            ExecutionProcessEvent::find_by_execution_process(&self.db.pool, run_id)
                .await?
                .iter()
                .filter(|e| {
                    e.event_type == ExecutionProcessEventType::ToolCall
                        && e.payload.0.get("tool").and_then(Value::as_str) == Some(REPORT_TOOL)
                        && e.payload.0.get("outcome").and_then(Value::as_str) == Some("handed_off")
                })
                .count(),
        )
    }

    /// The `report_problem` tool: checks the report, hands it to the triage
    /// agent in the background and answers at once.
    pub async fn hand_off(&self, run_id: Uuid, arguments: &Value) -> Result<String, String> {
        let report = parse_report(arguments)?;
        let pool = &self.db.pool;
        let db = |e: sqlx::Error| e.to_string();
        let run = ExecutionProcess::find_by_id(pool, run_id)
            .await
            .map_err(db)?
            .ok_or("Only coding runs can report problems")?;
        let source = match run.issue_id {
            Some(id) => Issue::find_by_id(pool, id).await.map_err(db)?,
            None => None,
        }
        .ok_or("This run does not work on a ticket, so there is no backlog to report to")?;
        let project = Project::find_by_id(pool, source.project_id)
            .await
            .map_err(db)?
            .ok_or("The ticket's project is gone")?;
        if self.handed_off(run_id).await.map_err(db)? >= MAX_REPORTS_PER_RUN {
            return Err(format!(
                "This run already reported {MAX_REPORTS_PER_RUN} problems, the most one run may. \
                 Mention further ones in your final message instead."
            ));
        }
        self.log(
            run_id,
            json!({"tool": REPORT_TOOL, "outcome": "handed_off", "title": report.title}),
        )
        .await;

        let origin = Origin {
            run_id,
            agent: run.agent_def_id.unwrap_or_else(|| "coding".into()),
            source,
            project,
        };
        let service = self.clone();
        tokio::spawn(async move { service.triage(origin, report).await });
        Ok(
            "Handed to the triage agent, which files it in the backlog unless a ticket already \
            covers it. Carry on with your task."
                .to_string(),
        )
    }

    async fn triage(&self, origin: Origin, report: ProblemReport) {
        let pool = &self.db.pool;
        let (tickets, statuses) = match (
            Issue::find_by_project(pool, origin.project.id).await,
            ProjectStatus::find_by_project(pool, origin.project.id).await,
        ) {
            (Ok(t), Ok(s)) => (t, s),
            (Err(e), _) | (_, Err(e)) => {
                tracing::error!("Triage of \"{}\" failed: {e}", report.title);
                return;
            }
        };
        let (_, result) = self
            .jobs
            .run(JobRequest {
                kind: JOB_KIND_TRIAGE,
                role: AgentRole::Triage,
                project: Some(origin.project.clone()),
                issue_id: Some(origin.source.id),
                context: vec![(
                    "tickets.md".to_string(),
                    ticket_list(&tickets, &statuses, &origin.project.id_prefix),
                )],
                task: triage_task(
                    &origin.project.name,
                    &origin.source_id(),
                    &origin.agent,
                    &report,
                ),
                repos: Vec::new(),
            })
            .await;
        let decision = match result {
            Ok(output) => parse_decision(&output.message)
                .ok_or_else(|| "the triage agent did not answer with a decision".to_string()),
            Err(e) => Err(e),
        };
        let outcome = match decision {
            Ok(decision) => self.apply(&origin, &report, &tickets, decision).await,
            Err(e) => {
                tracing::warn!("Triage of \"{}\": {e}; filing it directly", report.title);
                self.fallback(&origin, &report, &tickets, &statuses).await
            }
        };
        match outcome {
            Ok(payload) => self.log(origin.run_id, payload).await,
            Err(e) => tracing::error!("Filing reported problem \"{}\" failed: {e}", report.title),
        }
    }

    async fn apply(
        &self,
        origin: &Origin,
        report: &ProblemReport,
        tickets: &[Issue],
        decision: TriageDecision,
    ) -> Result<Value, String> {
        match decision {
            TriageDecision::Create {
                title,
                description,
                priority,
            } => {
                let cleaned = ProblemReport {
                    title,
                    description,
                    location: report.location.clone(),
                    priority: priority.or(report.priority),
                };
                self.create(origin, &cleaned).await
            }
            TriageDecision::Duplicate { ticket, note } => {
                let prefix = &origin.project.id_prefix;
                match tickets.iter().find(|t| {
                    t.internal_id(prefix).eq_ignore_ascii_case(&ticket)
                        || t.display_id(prefix).eq_ignore_ascii_case(&ticket)
                }) {
                    Some(existing) => self.comment(origin, report, existing, note).await,
                    // A made-up id must not lose the report.
                    None => self.create(origin, report).await,
                }
            }
            TriageDecision::Drop { reason } => Ok(json!({
                "tool": REPORT_TOOL, "outcome": "dropped", "title": report.title, "reason": reason
            })),
        }
    }

    /// Without the triage agent: comment on an open ticket with nearly the
    /// same title, else file a new one.
    async fn fallback(
        &self,
        origin: &Origin,
        report: &ProblemReport,
        tickets: &[Issue],
        statuses: &[ProjectStatus],
    ) -> Result<Value, String> {
        let open = |t: &Issue| {
            statuses
                .iter()
                .find(|s| s.id == t.status_id)
                .is_none_or(|s| s.kind != ProjectStatusKind::Done)
        };
        let same = tickets
            .iter()
            .filter(|t| t.id != origin.source.id && open(t))
            .map(|t| (t, title_overlap(&t.title, &report.title)))
            .filter(|(_, overlap)| *overlap >= SAME_TITLE)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match same {
            Some((existing, _)) => self.comment(origin, report, existing, None).await,
            None => self.create(origin, report).await,
        }
    }

    async fn create(&self, origin: &Origin, report: &ProblemReport) -> Result<Value, String> {
        let pool = &self.db.pool;
        let issue = Issue::create(
            pool,
            origin.project.id,
            &CreateIssue {
                title: report.title.clone(),
                description: Some(ticket_description(
                    &report.description,
                    report.location.as_deref(),
                    &origin.agent,
                    &origin.source_id(),
                )),
                status_id: None,
                priority: report.priority,
                sort_order: None,
                parent_issue_id: None,
                parent_issue_sort_order: None,
                external_id: None,
                mode: None,
                budget_usd: None,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        if let Err(e) = IssueRelationship::create(
            pool,
            origin.project.id,
            issue.id,
            &CreateIssueRelationship {
                related_issue_id: origin.source.id,
                relationship_type: IssueRelationshipType::Related,
            },
        )
        .await
        {
            tracing::warn!("Linking reported ticket {} failed: {e}", issue.id);
        }
        Ok(json!({
            "tool": REPORT_TOOL, "outcome": "created", "issue_id": issue.id, "title": issue.title
        }))
    }

    async fn comment(
        &self,
        origin: &Origin,
        report: &ProblemReport,
        existing: &Issue,
        note: Option<String>,
    ) -> Result<Value, String> {
        IssueComment::create(
            &self.db.pool,
            existing.id,
            &format!("agent:{}", origin.run_id),
            &CreateIssueComment {
                message: duplicate_comment(report, note.as_deref(), &origin.source_id()),
                parent_id: None,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(json!({
            "tool": REPORT_TOOL, "outcome": "commented", "issue_id": existing.id,
            "title": report.title
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_overlap_ignores_case_punctuation_and_short_words() {
        assert_eq!(
            title_overlap(
                "Ticket counter skips numbers",
                "ticket COUNTER skips numbers!"
            ),
            1.0
        );
        assert!(
            title_overlap("Counter skips numbers", "Counter skips numbers on retry") >= SAME_TITLE
        );
        assert!(title_overlap("Counter skips numbers", "Login button misaligned") < 0.1);
        assert_eq!(title_overlap("a b", "a b"), 0.0);
    }

    #[test]
    fn report_needs_title_and_description() {
        assert!(parse_report(&json!({"title": "x"})).is_err());
        assert!(parse_report(&json!({"title": " ", "description": "y"})).is_err());
        let report = parse_report(&json!({
            "title": "x", "description": "y", "file": "src/a.rs", "line": 12, "severity": "High"
        }))
        .unwrap();
        assert_eq!(report.location.as_deref(), Some("src/a.rs:12"));
        assert_eq!(report.priority, Some(IssuePriority::High));
        assert_eq!(priority(Some("critical")), Some(IssuePriority::Urgent));
        assert_eq!(priority(Some("whatever")), None);
    }

    #[test]
    fn description_names_place_and_finder() {
        let text = ticket_description("  Off by one.  ", Some("src/a.rs:12"), "coder", "KAN-3");
        assert!(text.starts_with("Off by one.\n\n**Where:** `src/a.rs:12`"));
        assert!(text.ends_with("_Found by the `coder` agent while working on KAN-3._"));
    }

    #[test]
    fn parses_triage_decisions() {
        assert_eq!(
            parse_decision(
                "Sure:\n```json\n{\"action\": \"duplicate\", \"ticket\": \"#KAN-12\", \"note\": null}\n```"
            ),
            Some(TriageDecision::Duplicate {
                ticket: "KAN-12".into(),
                note: None
            })
        );
        assert_eq!(
            parse_decision(
                r#"{"action": "create", "title": "T", "description": "D", "priority": "low"}"#
            ),
            Some(TriageDecision::Create {
                title: "T".into(),
                description: "D".into(),
                priority: Some(IssuePriority::Low)
            })
        );
        assert!(matches!(
            parse_decision(r#"{"action": "drop"}"#),
            Some(TriageDecision::Drop { .. })
        ));
        assert_eq!(
            parse_decision(r#"{"action": "create", "title": "T"}"#),
            None
        );
        assert_eq!(parse_decision("no json"), None);
    }

    #[test]
    fn task_carries_the_report() {
        let report = ProblemReport {
            title: "Counter skips".into(),
            description: "It skips.".into(),
            location: Some("src/a.rs:3".into()),
            priority: Some(IssuePriority::High),
        };
        let task = triage_task("Demo", "KAN-3", "coder", &report);
        assert!(task.contains("`coder` agent found a problem while working on KAN-3"));
        assert!(task.contains("Title: Counter skips\nWhere: src/a.rs:3\nSeverity: high\n"));
        assert!(task.ends_with("It skips.\n</report>\n"));
    }
}
