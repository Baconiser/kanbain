//! `request_setup`: a sandboxed agent that finds something missing (a
//! headless browser, a test runner, a CLI) asks for it instead of spelunking
//! through `/usr/bin` for an hour. The request lands in the project settings
//! next to the setup agent, plus a comment on the ticket and a notification.
//! The user hands the pending requests to the setup agent, which proposes
//! role-only setup commands for them ([`crate::setup_scan`]), or dismisses
//! them. Nothing gets installed mid-run: the agent is told to finish what it
//! can and say what it couldn't do; the next run of that role has the stuff.

use std::sync::Arc;

use db::{
    DBService,
    models::{
        execution_process::ExecutionProcess,
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::Issue,
        issue_comment::{CreateIssueComment, IssueComment},
        project::Project,
        setup_request::{CreateSetupRequest, SetupRequest},
    },
};
use executors::adapters::AgentRole;
use serde_json::{Value, json};
use services::services::{agent_library::AgentLibrary, container::ContainerService};
use uuid::Uuid;

use crate::{container::LocalContainerService, report::title_overlap};

pub const REQUEST_TOOL: &str = "request_setup";

/// Requests one run may make. Three missing things is already a pretty
/// clear "this sandbox can't do the job" signal.
pub const MAX_REQUESTS_PER_RUN: i64 = 3;
/// Word overlap (Jaccard) at which a pending request of the same role counts
/// as the same need.
const SAME_NEED: f64 = 0.6;
const MAX_NEED: usize = 200;
const MAX_TEXT: usize = 4000;

pub fn definition() -> Value {
    json!({
        "name": REQUEST_TOOL,
        "description": "Ask the project's owner to add something missing from your sandbox to \
            the project's setup: a tool, test runner, browser, CLI or system dependency your \
            task needs. Call it once per missing thing, as soon as you know it is missing, \
            instead of searching the filesystem or trying workarounds. It does NOT install \
            anything during this run: the owner reviews the request and the setup agent adds \
            it for later runs of your role. Returns at once.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "need": {
                    "type": "string",
                    "description": "What is missing, in a few words, e.g. \"Headless Chromium for Playwright\" or \"vitest\"."
                },
                "reason": {
                    "type": "string",
                    "description": "What you need it for and what you tried, e.g. \"Rendering checks for the WebGL canvas; `npx playwright test` finds no browser.\""
                },
                "suggested_commands": {
                    "type": "string",
                    "description": "Shell commands that would install it as the `node` user without root, if you know them, e.g. \"npx playwright install chromium\"."
                }
            },
            "required": ["need", "reason"]
        }
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupAsk {
    pub need: String,
    pub reason: String,
    pub suggestion: Option<String>,
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn parse_ask(arguments: &Value) -> Result<SetupAsk, String> {
    let need = text(arguments, "need").ok_or("`need` is required")?;
    let reason = text(arguments, "reason").ok_or("`reason` is required")?;
    Ok(SetupAsk {
        need: clip(need, MAX_NEED),
        reason: clip(reason, MAX_TEXT),
        suggestion: text(arguments, "suggested_commands").map(|s| clip(s, MAX_TEXT)),
    })
}

/// A pending request of the same role that already asks for this.
pub fn same_request<'a>(
    pending: &'a [SetupRequest],
    role: Option<AgentRole>,
    need: &str,
) -> Option<&'a SetupRequest> {
    pending.iter().find(|r| {
        r.role.0 == role
            && (r.need.eq_ignore_ascii_case(need) || title_overlap(&r.need, need) >= SAME_NEED)
    })
}

/// The ticket comment that tells the user what the agent is missing.
pub fn ticket_comment(agent: &str, ask: &SetupAsk) -> String {
    let mut comment = format!(
        "**Setup request from {agent}:** {}\n\n{}\n",
        ask.need, ask.reason
    );
    if let Some(suggestion) = &ask.suggestion {
        comment.push_str(&format!("\nSuggested:\n\n```sh\n{suggestion}\n```\n"));
    }
    comment.push_str(
        "\nReview it under Project settings → Setup agent: hand it to the setup agent or \
         dismiss it.",
    );
    comment
}

/// What the agent hears back.
fn answer(role: Option<AgentRole>) -> String {
    let who = match role {
        Some(role) => format!("{} runs", role.as_ref().replace('_', " ")),
        None => "later runs".to_string(),
    };
    format!(
        "Recorded. The project's owner reviews setup requests; once approved, it is set up for \
         {who} from the next run on, not in this one. Do not try to install it yourself. \
         Finish what you can without it and say in your final message what you could not do \
         because it is missing."
    )
}

#[derive(Clone)]
pub struct SetupRequests {
    db: DBService,
    agents: Arc<AgentLibrary>,
    container: LocalContainerService,
}

impl SetupRequests {
    pub fn new(db: DBService, agents: Arc<AgentLibrary>, container: LocalContainerService) -> Self {
        Self {
            db,
            agents,
            container,
        }
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

    /// The `request_setup` tool: records the request and answers at once.
    pub async fn request(&self, run_id: Uuid, arguments: &Value) -> Result<String, String> {
        let ask = parse_ask(arguments)?;
        let pool = &self.db.pool;
        let db = |e: sqlx::Error| e.to_string();
        let run = ExecutionProcess::find_by_id(pool, run_id)
            .await
            .map_err(db)?
            .ok_or("Only coding runs can request setup")?;
        let issue = match run.issue_id {
            Some(id) => Issue::find_by_id(pool, id).await.map_err(db)?,
            None => None,
        }
        .ok_or("This run does not work on a ticket, so there is no project to set up")?;
        let project = Project::find_by_id(pool, issue.project_id)
            .await
            .map_err(db)?
            .ok_or("The ticket's project is gone")?;
        if SetupRequest::count_for_run(pool, run_id)
            .await
            .map_err(db)?
            >= MAX_REQUESTS_PER_RUN
        {
            return Err(format!(
                "This run already made {MAX_REQUESTS_PER_RUN} setup requests, the most one run \
                 may. List anything else that is missing in your final message."
            ));
        }

        // An agent renamed or deleted since the run started just means we
        // can't say which role it was; the request still counts.
        let role = match &run.agent_def_id {
            Some(name) => self.agents.get_agent(name).await.ok().map(|d| d.role),
            None => None,
        };
        let agent = run
            .agent_def_id
            .clone()
            .unwrap_or_else(|| "an agent".into());

        let pending = SetupRequest::find_pending(pool, project.id)
            .await
            .map_err(db)?;
        if let Some(existing) = same_request(&pending, role, &ask.need) {
            self.log(
                run_id,
                json!({"tool": REQUEST_TOOL, "outcome": "duplicate", "need": ask.need,
                       "request_id": existing.id}),
            )
            .await;
            return Ok(format!(
                "\"{}\" is already requested and waits for the owner. {}",
                existing.need,
                answer(role)
            ));
        }

        let request = SetupRequest::create(
            pool,
            &CreateSetupRequest {
                project_id: project.id,
                issue_id: Some(issue.id),
                execution_process_id: Some(run_id),
                role,
                agent: Some(agent.clone()),
                need: ask.need.clone(),
                reason: ask.reason.clone(),
                suggestion: ask.suggestion.clone(),
            },
        )
        .await
        .map_err(db)?;
        self.log(
            run_id,
            json!({"tool": REQUEST_TOOL, "outcome": "requested", "need": ask.need,
                   "request_id": request.id}),
        )
        .await;

        if let Err(e) = IssueComment::create(
            pool,
            issue.id,
            &format!("agent:{run_id}"),
            &CreateIssueComment {
                message: ticket_comment(&agent, &ask),
                parent_id: None,
            },
        )
        .await
        {
            tracing::warn!("Setup request comment on {} failed: {e}", issue.id);
        }
        self.container
            .notification_service()
            .notify(
                "Sandbox setup requested",
                &format!(
                    "{agent} on {} needs {}. Review it in the project settings.",
                    issue.display_id(&project.id_prefix),
                    ask.need
                ),
                None,
            )
            .await;

        Ok(answer(role))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use db::models::setup_request::{RoleColumn, SetupRequestStatus};

    use super::*;

    fn pending(role: Option<AgentRole>, need: &str) -> SetupRequest {
        SetupRequest {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            issue_id: None,
            execution_process_id: None,
            role: RoleColumn(role),
            agent: None,
            need: need.into(),
            reason: String::new(),
            suggestion: None,
            status: SetupRequestStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        }
    }

    #[test]
    fn parses_and_clips_asks() {
        let ask = parse_ask(&json!({
            "need": "  vitest ",
            "reason": "unit tests",
            "suggested_commands": "  "
        }))
        .unwrap();
        assert_eq!(ask.need, "vitest");
        assert_eq!(ask.suggestion, None);

        let long = "x".repeat(MAX_NEED + 50);
        let ask = parse_ask(&json!({"need": long, "reason": "r"})).unwrap();
        assert_eq!(ask.need.chars().count(), MAX_NEED);

        assert!(parse_ask(&json!({"need": "vitest"})).is_err());
        assert!(parse_ask(&json!({"reason": "r", "need": " "})).is_err());
    }

    #[test]
    fn same_need_same_role_is_a_duplicate() {
        let list = [
            pending(Some(AgentRole::Tester), "Headless Chromium for Playwright"),
            pending(Some(AgentRole::Coder), "vitest"),
        ];
        assert!(
            same_request(
                &list,
                Some(AgentRole::Tester),
                "headless chromium for playwright tests"
            )
            .is_some()
        );
        // Same need, other role: the coder setup doesn't help a tester.
        assert!(same_request(&list, Some(AgentRole::Tester), "vitest").is_none());
        assert!(same_request(&list, Some(AgentRole::Coder), "VITEST").is_some());
        assert!(same_request(&list, Some(AgentRole::Tester), "Firefox").is_none());
    }

    #[test]
    fn comment_and_answer_say_what_happens_next() {
        let ask = SetupAsk {
            need: "Chromium".into(),
            reason: "WebGL checks".into(),
            suggestion: Some("npx playwright install chromium".into()),
        };
        let comment = ticket_comment("tester", &ask);
        assert!(comment.starts_with("**Setup request from tester:** Chromium"));
        assert!(comment.contains("```sh\nnpx playwright install chromium\n```"));
        assert!(comment.contains("Project settings"));

        let reply = answer(Some(AgentRole::ConflictResolver));
        assert!(reply.contains("conflict resolver runs"));
        assert!(reply.contains("not in this one"));
    }
}
