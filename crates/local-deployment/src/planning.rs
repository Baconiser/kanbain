//! The planning flow and approval gate (Phase 4, D20, D21).
//!
//! Assigning a ticket to the AI (or dragging it to a Planning status)
//! starts a planner run: a sandboxed session of the `planner` agent in the
//! ticket's own workspace, with the ticket, prior work and `related.md` in
//! `/kanbain/context`. The planner hands in `/kanbain/out/plan.json`, or
//! `questions.md` when it needs answers first. Questions become an agent
//! comment; the user's reply resumes the same session. A plan becomes a
//! draft version that the user approves, edits (a new version) or sends back
//! with feedback (the planner revises it in the same session). No coding
//! starts before approval.

use std::sync::{Arc, OnceLock};

use db::{
    DBService,
    models::{
        coding_agent_turn::CodingAgentTurn,
        execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::{Issue, IssueMode, UpdateIssue},
        issue_comment::{CreateIssueComment, IssueComment, USER_AUTHOR},
        issue_event::{IssueEvent, IssueEventType},
        issue_history::{HistoryStatus, IssueHistory},
        plan::{IssuePlanning, Plan, PlanDocument, PlanSource, PlanStatus, PlanningStatus},
        project::{Project, ProjectRepo},
        project_status::{ProjectStatus, ProjectStatusKind},
        requests::WorkspaceRepoInput,
        session::{CreateSession, Session},
        workspace::{CreateWorkspace, Workspace},
    },
};
use embeddings::{SearchService, service::Related};
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType, coding_agent_follow_up::CodingAgentFollowUpRequest,
        coding_agent_initial::CodingAgentInitialRequest,
    },
    adapters::{AgentRole, executor_for, headless::extract_json_object},
    model_selector::PermissionPolicy,
    profile::ExecutorConfig,
};
use git::GitService;
use serde::Serialize;
use serde_json::json;
use services::services::{agent_library::AgentLibrary, container::ContainerService};
use ts_rs::TS;
use utils::ticket_id::ticket_branch_name;
use uuid::Uuid;
use workspace_manager::WorkspaceManager;

use crate::{container::LocalContainerService, jobs::AgentJobs};

/// `related.md` cap (embedding plan §5: top 5 tickets, 10 code locations,
/// about 8 KB).
const RELATED_MAX_BYTES: usize = 8 * 1024;
const RELATED_TICKETS: usize = 5;
const RELATED_CODE: usize = 10;
/// Automatic "your plan.json is invalid, fix it" follow-ups per planning.
const MAX_REPAIRS: i64 = 1;
/// How long a planner run waits for a prior-work summary still being written.
const HISTORY_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
/// Largest questions comment.
const MAX_QUESTIONS_CHARS: usize = 8000;

#[derive(Debug, thiserror::Error)]
pub enum PlanningError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Ticket not found")]
    NotFound,
    #[error("A planner run for this ticket is already in progress")]
    AlreadyPlanning,
    #[error("{0}")]
    Invalid(String),
}

/// Why a planner run starts; decides the task message.
#[derive(Debug, Clone)]
pub enum PlanTrigger {
    /// Assigned to the AI or dragged to Planning.
    Assigned,
    /// The user answered the planner's questions.
    Answer { message: String },
    /// The user sent a plan version back.
    Feedback { version: i64, feedback: String },
    /// The planner's `plan.json` didn't validate.
    Repair { errors: Vec<String> },
}

impl PlanTrigger {
    fn reason(&self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Answer { .. } => "answer",
            Self::Feedback { .. } => "feedback",
            Self::Repair { .. } => "repair",
        }
    }
}

/// What the ticket panel shows about planning.
#[derive(Debug, Clone, Serialize, TS)]
pub struct PlanningView {
    pub planning: Option<IssuePlanning>,
    pub plans: Vec<Plan>,
}

/// The `plan.json` contract, as the planner sees it.
pub const PLAN_SCHEMA: &str = r#"{
  "subtasks": [
    {"id": "1", "title": "…", "description": "what to do and why",
     "files": ["repo/path/likely/touched.rs"], "depends_on": []}
  ],
  "acceptance_criteria": ["…"],
  "assumptions": ["…"],
  "test_strategy": "…",
  "checkpoints": [{"after_subtasks": ["1"], "what": "what the tester checks then"}]
}"#;

/// The first message of a planner session.
pub fn planner_task(id: &str, title: &str, mode: IssueMode, prior_work: bool) -> String {
    let mut task = format!(
        "Plan the ticket {id}: \"{title}\".\n\n\
         Read these first (read-only):\n\
         - /kanbain/context/ticket.md: the ticket.\n"
    );
    if prior_work {
        task.push_str(
            "- /kanbain/context/prior_work.md: what earlier commits for this ticket did.\n",
        );
    }
    task.push_str(
        "- /kanbain/context/related.md: similar tickets (with their status, runs and the \
         files they changed) and code locations found by search. Check it before exploring \
         the code, and cite an earlier ticket in your assumptions when you build on it.\n\n",
    );
    match mode {
        IssueMode::Ask => task.push_str(
            "Mode: ask. If something important is unclear (behaviour, scope, a choice only \
             the user can make), write your questions as a numbered Markdown list to \
             /kanbain/out/questions.md and stop without writing plan.json. The user's \
             answers will come back in this session. Don't ask about what you can find out \
             from the code.\n\n",
        ),
        IssueMode::Yolo => task.push_str(
            "Mode: yolo. Do not ask questions. Decide yourself and list every decision you \
             made without the user in `assumptions`.\n\n",
        ),
    }
    task.push_str(&plan_contract());
    task
}

fn plan_contract() -> String {
    format!(
        "When the plan is ready, write it to /kanbain/out/plan.json in exactly this shape:\n\
         {PLAN_SCHEMA}\n\n\
         Rules:\n\
         - Subtasks are units one engineer can implement and commit on their own. Give each \
         the files it likely touches: subtasks with disjoint files run in parallel, \
         overlapping ones in order. `depends_on` lists ids of subtasks that must finish \
         first.\n\
         - Acceptance criteria are checkable from outside; the tester writes black-box \
         tests from them.\n\
         - Checkpoints say after which subtasks the tester runs and what it checks.\n\
         - Do not change code, create branches or commit. Only write files under \
         /kanbain/out/.\n"
    )
}

fn answer_task(message: &str) -> String {
    format!(
        "The user answered your questions:\n\n{}\n\n\
         Continue planning. Write /kanbain/out/plan.json as described before, or \
         /kanbain/out/questions.md again if you still need answers.",
        message.trim()
    )
}

fn feedback_task(version: i64, feedback: &str) -> String {
    format!(
        "The user sent plan version {version} back with this feedback:\n\n{}\n\n\
         The version they reviewed (including any edits they made) is \
         /kanbain/context/current_plan.json. Write a revised plan to \
         /kanbain/out/plan.json in the same shape.",
        feedback.trim()
    )
}

fn repair_task(errors: &[String]) -> String {
    let list: Vec<String> = errors.iter().map(|e| format!("- {e}")).collect();
    format!(
        "Your /kanbain/out/plan.json could not be used:\n{}\n\n\
         Write it again, fixed, in this shape:\n{PLAN_SCHEMA}",
        list.join("\n")
    )
}

/// What a finished planner run handed in.
#[derive(Debug, Default, PartialEq)]
pub struct Harvest {
    pub plan: Option<Result<PlanDocument, Vec<String>>>,
    pub questions: Option<String>,
}

/// Reads `plan.json` and `questions.md` from the run's harvested out files.
/// When the planner wrote no `plan.json` (an older agent without the out
/// fence, or a runtime that refused the write), a JSON plan in its final
/// message counts too.
pub fn read_harvest(
    files: &serde_json::Map<String, serde_json::Value>,
    final_message: Option<&str>,
) -> Harvest {
    let file = |name: &str| {
        files
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let parse = |text: &str| match extract_json_object(text) {
        Some(value) => PlanDocument::from_value(value),
        None => Err(vec!["plan.json is not a JSON object.".to_string()]),
    };
    let plan = match file("plan.json") {
        Some(text) => Some(parse(text)),
        None => final_message
            .and_then(extract_json_object)
            .filter(|v| v.get("subtasks").is_some())
            .map(PlanDocument::from_value),
    };
    let questions = file("questions.md").map(|q| {
        let mut q = q.to_string();
        if q.chars().count() > MAX_QUESTIONS_CHARS {
            q = q.chars().take(MAX_QUESTIONS_CHARS).collect::<String>() + "…";
        }
        q
    });
    Harvest { plan, questions }
}

/// `related.md` from a ticket's related tickets and code (E22, D43), capped
/// at [`RELATED_MAX_BYTES`]. `tickets` gives each related ticket's display
/// id, title and status name.
pub fn render_related(
    id: &str,
    related: &Related,
    tickets: &dyn Fn(Uuid) -> Option<(String, String, String)>,
) -> String {
    let mut out = format!(
        "# Related work for {id}\n\n\
         Found by KanbAIn's search: earlier tickets like this one and code that likely \
         matters. Locations, not contents: read only the line ranges you need.\n"
    );
    let push = |out: &mut String, entry: String| {
        if out.len() + entry.len() > RELATED_MAX_BYTES {
            return false;
        }
        out.push_str(&entry);
        true
    };
    let one_line = |text: &str, max: usize| {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > max {
            format!("{}…", flat.chars().take(max).collect::<String>())
        } else {
            flat
        }
    };

    let tickets_list: Vec<_> = related
        .tickets
        .iter()
        .filter_map(|t| tickets(t.issue_id).map(|info| (t, info)))
        .take(RELATED_TICKETS)
        .collect();
    if !tickets_list.is_empty() {
        out.push_str("\n## Similar tickets\n");
        for (ticket, (tid, title, status)) in tickets_list {
            let mut entry = format!("\n- {tid} \"{title}\" ({status})\n");
            if !ticket.changed.is_empty() {
                entry.push_str(&format!("  Changed: {}\n", ticket.changed.join("; ")));
            }
            if ticket.kinds.iter().any(|k| k == "run") {
                entry.push_str("  Has earlier agent runs.\n");
            }
            entry.push_str(&format!("  > {}\n", one_line(&ticket.excerpt, 240)));
            if !push(&mut out, entry) {
                return out;
            }
        }
    }

    let code: Vec<_> = related.code.iter().take(RELATED_CODE).collect();
    if !code.is_empty() {
        out.push_str("\n## Code locations\n\n");
        for location in code {
            let hit = &location.hit;
            let Some(path) = &hit.path else { continue };
            let lines = match (hit.start_line, hit.end_line) {
                (Some(a), Some(b)) => format!(":{a}-{b}"),
                _ => String::new(),
            };
            let mut entry = format!("- {path}{lines}");
            if let Some(symbol) = hit.symbol.as_deref().filter(|s| !s.is_empty()) {
                entry.push_str(&format!(" `{symbol}`"));
            }
            let mut changed_by: Vec<String> = Vec::new();
            for tid in location
                .changed_by
                .iter()
                .filter_map(|id| tickets(*id).map(|(tid, _, _)| tid))
            {
                if !changed_by.contains(&tid) {
                    changed_by.push(tid);
                }
            }
            if !changed_by.is_empty() {
                entry.push_str(&format!(" (changed by {})", changed_by.join(", ")));
            }
            entry.push('\n');
            if !push(&mut out, entry) {
                return out;
            }
        }
    }
    if related.tickets.is_empty() && related.code.is_empty() {
        out.push_str("\nNothing related was found.\n");
    }
    out
}

#[derive(Clone)]
pub struct PlanningService {
    db: DBService,
    container: LocalContainerService,
    workspace_manager: WorkspaceManager,
    git: GitService,
    library: Arc<AgentLibrary>,
    jobs: AgentJobs,
    search: Arc<OnceLock<SearchService>>,
}

impl PlanningService {
    pub fn new(
        db: DBService,
        container: LocalContainerService,
        workspace_manager: WorkspaceManager,
        git: GitService,
        library: Arc<AgentLibrary>,
        jobs: AgentJobs,
    ) -> Self {
        Self {
            db,
            container,
            workspace_manager,
            git,
            library,
            jobs,
            search: Arc::default(),
        }
    }

    pub fn set_search(&self, search: SearchService) {
        let _ = self.search.set(search);
    }

    pub async fn view(&self, issue_id: Uuid) -> Result<PlanningView, PlanningError> {
        let pool = &self.db.pool;
        Ok(PlanningView {
            planning: IssuePlanning::find(pool, issue_id).await?,
            plans: Plan::find_by_issue(pool, issue_id).await?,
        })
    }

    async fn event(
        &self,
        issue_id: Uuid,
        kind: IssueEventType,
        actor: &str,
        payload: serde_json::Value,
    ) {
        if let Err(e) =
            IssueEvent::create(&self.db.pool, issue_id, kind, actor, &payload, None).await
        {
            tracing::warn!("Recording a planning event for {issue_id} failed: {e}");
        }
    }

    /// Moves the ticket to its project's first status of `kind`, if it has
    /// one and isn't there yet.
    async fn move_to(&self, issue: &Issue, kind: ProjectStatusKind) {
        let pool = &self.db.pool;
        let Ok(Some(status)) =
            ProjectStatus::find_first_of_kind(pool, issue.project_id, kind).await
        else {
            return;
        };
        if status.id == issue.status_id {
            return;
        }
        let update = UpdateIssue {
            status_id: Some(status.id),
            ..Default::default()
        };
        if let Err(e) = Issue::update(pool, issue.id, &update).await {
            tracing::warn!("Moving ticket {} to {kind:?} failed: {e}", issue.id);
        }
    }

    async fn fail(&self, issue_id: Uuid, error: &str) {
        let pool = &self.db.pool;
        if let Err(e) =
            IssuePlanning::set_status(pool, issue_id, PlanningStatus::Failed, Some(error)).await
        {
            tracing::warn!("Recording a planning failure for {issue_id} failed: {e}");
        }
        self.event(
            issue_id,
            IssueEventType::PlanningFailed,
            "system",
            json!({"error": error}),
        )
        .await;
    }

    /// Whether moving a ticket into a status should start planning: it
    /// entered a Planning status and nothing is planned or planning yet.
    pub async fn should_start_on_status(
        &self,
        before: &Issue,
        after: &Issue,
    ) -> Result<bool, PlanningError> {
        if before.status_id == after.status_id {
            return Ok(false);
        }
        let pool = &self.db.pool;
        let Some(status) = ProjectStatus::find_by_id(pool, after.status_id).await? else {
            return Ok(false);
        };
        if status.kind != ProjectStatusKind::Planning {
            return Ok(false);
        }
        Ok(match IssuePlanning::find(pool, after.id).await? {
            None => true,
            Some(p) => matches!(p.status, PlanningStatus::Idle | PlanningStatus::Failed),
        })
    }

    /// Starts a planner run in the background. Errors found before the run
    /// is claimed are returned; later ones are recorded on the planning.
    pub async fn start(
        &self,
        issue_id: Uuid,
        trigger: PlanTrigger,
    ) -> Result<IssuePlanning, PlanningError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, issue_id)
            .await?
            .ok_or(PlanningError::NotFound)?;
        if !IssuePlanning::begin(pool, issue_id).await? {
            return Err(PlanningError::AlreadyPlanning);
        }
        if matches!(trigger, PlanTrigger::Assigned) {
            IssuePlanning::set_repair_attempts(pool, issue_id, 0).await?;
        }
        self.event(
            issue_id,
            IssueEventType::PlanningStarted,
            "system",
            json!({"reason": trigger.reason()}),
        )
        .await;
        self.move_to(&issue, ProjectStatusKind::Planning).await;

        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.run(issue_id, trigger).await {
                tracing::warn!("Planner run for ticket {issue_id} failed to start: {e}");
                service.fail(issue_id, &e.to_string()).await;
            }
        });
        IssuePlanning::find(pool, issue_id)
            .await?
            .ok_or(PlanningError::NotFound)
    }

    /// The planner's workspace: the ticket's earlier one, or a new one on
    /// the ticket branch with every repo of the project.
    async fn workspace_for(
        &self,
        issue: &Issue,
        project: &Project,
        planning: &IssuePlanning,
    ) -> Result<Workspace, PlanningError> {
        let pool = &self.db.pool;
        if let Some(id) = planning.workspace_id
            && let Some(workspace) = Workspace::find_by_id(pool, id).await?
        {
            self.container
                .ensure_container_exists(&workspace)
                .await
                .map_err(|e| PlanningError::Invalid(e.to_string()))?;
            return Ok(workspace);
        }

        let repos = ProjectRepo::find_repos_for_project(pool, project.id).await?;
        if repos.is_empty() {
            return Err(PlanningError::Invalid(
                "The project has no repositories to plan against; link one in the project \
                 settings"
                    .to_string(),
            ));
        }
        let base = ticket_branch_name(&issue.display_id(&project.id_prefix), &issue.title);
        let mut branch = base.clone();
        let mut n = 2;
        while Workspace::branch_in_use(pool, &branch).await? {
            branch = format!("{base}-{n}");
            n += 1;
        }
        let workspace = Workspace::create(
            pool,
            &CreateWorkspace {
                branch,
                name: Some(format!("Plan {}", issue.display_id(&project.id_prefix))),
                issue_id: Some(issue.id),
            },
            Uuid::new_v4(),
        )
        .await
        .map_err(|e| PlanningError::Invalid(e.to_string()))?;
        let mut managed = self
            .workspace_manager
            .load_managed_workspace(workspace)
            .await?;
        for repo in &repos {
            let target_branch = match &repo.default_target_branch {
                Some(branch) => branch.clone(),
                None => self
                    .git
                    .get_current_branch(&repo.path)
                    .map_err(|e| PlanningError::Invalid(format!("{}: {e}", repo.name)))?,
            };
            managed
                .add_repository(
                    &WorkspaceRepoInput {
                        repo_id: repo.id,
                        target_branch,
                    },
                    &self.git,
                )
                .await
                .map_err(|e| PlanningError::Invalid(e.to_string()))?;
        }
        let workspace = managed.workspace.clone();
        self.container
            .create(&workspace)
            .await
            .map_err(|e| PlanningError::Invalid(e.to_string()))?;
        Ok(Workspace::find_by_id(pool, workspace.id)
            .await?
            .unwrap_or(workspace))
    }

    /// Waits (bounded) for a prior-work summary still being written, so the
    /// planner sees it (3b leftover).
    async fn wait_for_history(&self, issue_id: Uuid) {
        let deadline = tokio::time::Instant::now() + HISTORY_WAIT;
        loop {
            match IssueHistory::find(&self.db.pool, issue_id).await {
                Ok(Some(h)) if h.status == HistoryStatus::Analyzing => {}
                _ => return,
            }
            if tokio::time::Instant::now() > deadline {
                tracing::info!(
                    "Planning {issue_id} without the prior-work summary still being written"
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    async fn run(&self, issue_id: Uuid, trigger: PlanTrigger) -> Result<(), PlanningError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, issue_id)
            .await?
            .ok_or(PlanningError::NotFound)?;
        let project = Project::find_by_id(pool, issue.project_id)
            .await?
            .ok_or(PlanningError::NotFound)?;
        let planning = IssuePlanning::find(pool, issue_id)
            .await?
            .ok_or(PlanningError::NotFound)?;
        if !self.container.sandbox().enabled().await {
            return Err(PlanningError::Invalid(
                "The planner runs in the sandbox; turn it on in Settings → Sandbox".to_string(),
            ));
        }
        let agent_name = self
            .jobs
            .agent_name_for(AgentRole::Planner)
            .await
            .map_err(PlanningError::Invalid)?;
        let agent = self
            .library
            .get_agent(&agent_name)
            .await
            .map_err(|e| PlanningError::Invalid(format!("Agent `{agent_name}`: {e}")))?;
        self.wait_for_history(issue_id).await;

        let workspace = self.workspace_for(&issue, &project, &planning).await?;
        let mut executor_config = ExecutorConfig::new(executor_for(agent.runtime));
        executor_config.agent_definition = Some(agent.name.clone());
        executor_config.permission_policy = Some(PermissionPolicy::Auto);

        // Answers, feedback and repairs resume the planner's session.
        let session = match planning.session_id {
            Some(id) if !matches!(trigger, PlanTrigger::Assigned) => {
                Session::find_by_id(pool, id).await?
            }
            _ => None,
        };
        let resume = match &session {
            Some(session) => CodingAgentTurn::find_latest_session_info(pool, session.id).await?,
            None => None,
        };
        let session = match session {
            Some(session) => session,
            None => Session::create(
                pool,
                &CreateSession {
                    executor: Some(executor_config.executor.to_string()),
                    name: Some("Planner".to_string()),
                },
                Uuid::new_v4(),
                workspace.id,
            )
            .await
            .map_err(|e| PlanningError::Invalid(e.to_string()))?,
        };

        let prior_work = IssueHistory::find(pool, issue_id)
            .await?
            .and_then(|h| h.prior_work)
            .is_some();
        let id = issue.display_id(&project.id_prefix);
        let first_task = planner_task(&id, &issue.title, issue.mode, prior_work);
        let followup = match &trigger {
            PlanTrigger::Assigned => None,
            PlanTrigger::Answer { message } => Some(answer_task(message)),
            PlanTrigger::Feedback { version, feedback } => Some(feedback_task(*version, feedback)),
            PlanTrigger::Repair { errors } => Some(repair_task(errors)),
        };
        let working_dir = session
            .agent_working_dir
            .as_ref()
            .filter(|dir| !dir.is_empty())
            .cloned();
        // No setup or cleanup scripts: the planner only reads, and a chained
        // script would still be running when the next planner turn starts.
        let action_type = match (resume, followup) {
            (Some(info), Some(prompt)) => {
                ExecutorActionType::CodingAgentFollowUpRequest(CodingAgentFollowUpRequest {
                    prompt,
                    session_id: info.session_id,
                    reset_to_message_id: None,
                    executor_config,
                    working_dir,
                })
            }
            // No session to resume: start over, with the follow-up's context.
            (_, followup) => {
                let prompt = match followup {
                    Some(extra) => format!("{first_task}\n---\n{extra}"),
                    None => first_task,
                };
                ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                    prompt,
                    executor_config,
                    working_dir,
                })
            }
        };
        let execution = self
            .container
            .start_execution(
                &workspace,
                &session,
                &ExecutorAction::new(action_type, None),
                &ExecutionProcessRunReason::CodingAgent,
            )
            .await
            .map_err(|e| PlanningError::Invalid(e.to_string()))?;
        IssuePlanning::set_run(pool, issue_id, workspace.id, session.id, execution.id).await?;
        Ok(())
    }

    /// Context files for a planner run of this ticket: `related.md` and,
    /// when there is one, the newest plan as `current_plan.json`.
    pub async fn context_files(&self, issue: &Issue) -> Vec<(String, String)> {
        let pool = &self.db.pool;
        let mut files = Vec::new();
        let prefix = match Project::find_by_id(pool, issue.project_id).await {
            Ok(Some(p)) => p.id_prefix,
            _ => "KAN".to_string(),
        };
        let id = issue.display_id(&prefix);
        let related = match self.search.get() {
            Some(search) => match search.related(issue, RELATED_TICKETS * 2).await {
                Ok(related) => {
                    let mut infos = std::collections::HashMap::new();
                    let ids = related
                        .tickets
                        .iter()
                        .map(|t| t.issue_id)
                        .chain(related.code.iter().flat_map(|c| c.changed_by.clone()));
                    for other in ids {
                        if infos.contains_key(&other) {
                            continue;
                        }
                        // Search is per project (D47); a repo shared with
                        // another project must not leak its tickets here.
                        if let Ok(Some(i)) = Issue::find_by_id(pool, other).await
                            && i.project_id == issue.project_id
                        {
                            let status = ProjectStatus::find_by_id(pool, i.status_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|s| s.name)
                                .unwrap_or_default();
                            infos.insert(other, (i.display_id(&prefix), i.title, status));
                        }
                    }
                    render_related(&id, &related, &|other| infos.get(&other).cloned())
                }
                Err(e) => {
                    tracing::warn!("Related search for {id} failed: {e}");
                    format!("# Related work for {id}\n\nSearch failed: {e}\n")
                }
            },
            None => format!("# Related work for {id}\n\nSearch is not available.\n"),
        };
        files.push(("related.md".to_string(), related));
        if let Ok(Some(plan)) = Plan::latest(pool, issue.id).await
            && let Ok(json) = serde_json::to_string_pretty(&plan.plan.0)
        {
            files.push(("current_plan.json".to_string(), json));
        }
        files
    }

    /// Called when a coding run ends. If it was a ticket's current planner
    /// run, its results move the planning on.
    pub async fn on_run_finished(&self, execution_id: Uuid) {
        if let Err(e) = self.harvest(execution_id).await {
            tracing::warn!("Reading planner run {execution_id} failed: {e}");
        }
    }

    async fn harvest(&self, execution_id: Uuid) -> Result<(), PlanningError> {
        let pool = &self.db.pool;
        let Some(planning) = IssuePlanning::find_by_run(pool, execution_id).await? else {
            return Ok(());
        };
        if planning.status != PlanningStatus::Planning {
            return Ok(());
        }
        let issue_id = planning.issue_id;
        let actor = format!("agent:{execution_id}");
        let Some(process) = ExecutionProcess::find_by_id(pool, execution_id).await? else {
            return Ok(());
        };
        match process.status {
            ExecutionProcessStatus::Completed => {}
            ExecutionProcessStatus::Killed => {
                self.fail(issue_id, "The planner run was stopped").await;
                return Ok(());
            }
            _ => {
                self.fail(
                    issue_id,
                    "The planner run failed; see its log in the workspace",
                )
                .await;
                return Ok(());
            }
        }

        let files = ExecutionProcessEvent::find_by_execution_process(pool, execution_id)
            .await?
            .into_iter()
            .filter(|e| e.event_type == ExecutionProcessEventType::Sandbox)
            .filter_map(|e| {
                (e.payload.0.get("action")?.as_str()? == "harvest")
                    .then(|| e.payload.0.get("files")?.as_object().cloned())
                    .flatten()
            })
            .next_back()
            .unwrap_or_default();
        let final_message = CodingAgentTurn::find_by_execution_process_id(pool, execution_id)
            .await?
            .and_then(|t| t.summary);
        let harvest = read_harvest(&files, final_message.as_deref());
        let Some(issue) = Issue::find_by_id(pool, issue_id).await? else {
            return Ok(());
        };

        if let Some(questions) = &harvest.questions {
            IssueComment::create(
                pool,
                issue_id,
                &actor,
                &CreateIssueComment {
                    message: format!("**Questions from the planner**\n\n{questions}"),
                    parent_id: None,
                },
            )
            .await?;
            self.event(
                issue_id,
                IssueEventType::PlanQuestions,
                &actor,
                json!({"count": questions.lines().filter(|l| is_list_item(l)).count()}),
            )
            .await;
        }

        match harvest.plan {
            Some(Ok(plan)) => {
                let plan = Plan::create(
                    pool,
                    issue_id,
                    &plan,
                    PlanSource::Planner,
                    Some(execution_id),
                )
                .await?;
                self.event(
                    issue_id,
                    IssueEventType::PlanVersion,
                    &actor,
                    json!({"version": plan.version, "source": "planner",
                           "subtasks": plan.plan.0.subtasks.len()}),
                )
                .await;
                IssuePlanning::set_status(pool, issue_id, PlanningStatus::AwaitingApproval, None)
                    .await?;
                self.move_to(&issue, ProjectStatusKind::AwaitingApproval)
                    .await;
                self.notify(
                    &issue,
                    "Plan ready for approval",
                    &format!(
                        "Plan v{} for \"{}\" is waiting for your approval.",
                        plan.version, issue.title
                    ),
                )
                .await;
            }
            Some(Err(errors)) if planning.repair_attempts < MAX_REPAIRS => {
                IssuePlanning::set_repair_attempts(pool, issue_id, planning.repair_attempts + 1)
                    .await?;
                // Leave `planning` so the repair run can claim it.
                IssuePlanning::set_status(pool, issue_id, PlanningStatus::Idle, None).await?;
                self.start(issue_id, PlanTrigger::Repair { errors }).await?;
            }
            Some(Err(errors)) => {
                self.fail(
                    issue_id,
                    &format!("The planner's plan.json is invalid: {}", errors.join(" ")),
                )
                .await;
            }
            None if harvest.questions.is_some() => {
                IssuePlanning::set_status(pool, issue_id, PlanningStatus::WaitingForAnswer, None)
                    .await?;
                self.notify(
                    &issue,
                    "The planner has questions",
                    &format!("Answer them on \"{}\" to continue planning.", issue.title),
                )
                .await;
            }
            None => {
                self.fail(
                    issue_id,
                    "The planner finished without writing plan.json or questions.md",
                )
                .await;
            }
        }
        Ok(())
    }

    async fn notify(&self, issue: &Issue, title: &str, message: &str) {
        let workspace = IssuePlanning::find(&self.db.pool, issue.id)
            .await
            .ok()
            .flatten()
            .and_then(|p| p.workspace_id);
        self.container
            .notification_service()
            .notify(title, message, workspace)
            .await;
    }

    /// A user comment on a ticket whose planner waits for answers resumes
    /// the planner with it.
    pub async fn on_comment(&self, comment: &IssueComment) -> Result<(), PlanningError> {
        if comment.author != USER_AUTHOR {
            return Ok(());
        }
        match IssuePlanning::find(&self.db.pool, comment.issue_id).await? {
            Some(p) if p.status == PlanningStatus::WaitingForAnswer => {}
            _ => return Ok(()),
        }
        self.start(
            comment.issue_id,
            PlanTrigger::Answer {
                message: comment.message.clone(),
            },
        )
        .await?;
        Ok(())
    }

    async fn latest_draft(&self, plan_id: Uuid) -> Result<Plan, PlanningError> {
        let pool = &self.db.pool;
        let plan = Plan::find_by_id(pool, plan_id)
            .await?
            .ok_or(PlanningError::NotFound)?;
        let latest = Plan::latest(pool, plan.issue_id).await?;
        if latest.as_ref().map(|l| l.id) != Some(plan.id) {
            return Err(PlanningError::Invalid(
                "Only the newest plan version can be changed".to_string(),
            ));
        }
        if plan.status != PlanStatus::Draft {
            return Err(PlanningError::Invalid(format!(
                "Plan v{} is already {}",
                plan.version, plan.status
            )));
        }
        if let Some(p) = IssuePlanning::find(pool, plan.issue_id).await?
            && p.status == PlanningStatus::Planning
        {
            return Err(PlanningError::AlreadyPlanning);
        }
        Ok(plan)
    }

    /// Approves the newest plan; the ticket moves to In progress (D21).
    pub async fn approve(&self, plan_id: Uuid) -> Result<Plan, PlanningError> {
        let pool = &self.db.pool;
        let plan = self.latest_draft(plan_id).await?;
        Plan::decide(pool, plan.id, PlanStatus::Approved, None).await?;
        sqlx::query("INSERT OR IGNORE INTO issue_planning (issue_id) VALUES (?)")
            .bind(plan.issue_id)
            .execute(pool)
            .await?;
        IssuePlanning::set_status(pool, plan.issue_id, PlanningStatus::Approved, None).await?;
        self.event(
            plan.issue_id,
            IssueEventType::PlanApproved,
            USER_AUTHOR,
            json!({"version": plan.version}),
        )
        .await;
        if let Some(issue) = Issue::find_by_id(pool, plan.issue_id).await? {
            self.move_to(&issue, ProjectStatusKind::InProgress).await;
        }
        Plan::find_by_id(pool, plan.id)
            .await?
            .ok_or(PlanningError::NotFound)
    }

    /// Saves the user's edit of the newest plan as a new version.
    pub async fn edit(&self, plan_id: Uuid, plan: PlanDocument) -> Result<Plan, PlanningError> {
        let pool = &self.db.pool;
        let current = self.latest_draft(plan_id).await?;
        plan.validate()
            .map_err(|errors| PlanningError::Invalid(errors.join(" ")))?;
        if plan == current.plan.0 {
            return Err(PlanningError::Invalid("Nothing changed".to_string()));
        }
        let created = Plan::create(pool, current.issue_id, &plan, PlanSource::Edited, None).await?;
        self.event(
            current.issue_id,
            IssueEventType::PlanVersion,
            USER_AUTHOR,
            json!({"version": created.version, "source": "edited",
                   "subtasks": plan.subtasks.len()}),
        )
        .await;
        sqlx::query("INSERT OR IGNORE INTO issue_planning (issue_id) VALUES (?)")
            .bind(current.issue_id)
            .execute(pool)
            .await?;
        IssuePlanning::set_status(
            pool,
            current.issue_id,
            PlanningStatus::AwaitingApproval,
            None,
        )
        .await?;
        Ok(created)
    }

    /// Sends the newest plan back; the planner revises it in its session.
    pub async fn send_back(&self, plan_id: Uuid, feedback: &str) -> Result<Plan, PlanningError> {
        let pool = &self.db.pool;
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(PlanningError::Invalid(
                "Say what the planner should change".to_string(),
            ));
        }
        let plan = self.latest_draft(plan_id).await?;
        Plan::decide(pool, plan.id, PlanStatus::Rejected, Some(feedback)).await?;
        self.event(
            plan.issue_id,
            IssueEventType::PlanRejected,
            USER_AUTHOR,
            json!({"version": plan.version, "feedback": feedback}),
        )
        .await;
        sqlx::query("INSERT OR IGNORE INTO issue_planning (issue_id) VALUES (?)")
            .bind(plan.issue_id)
            .execute(pool)
            .await?;
        self.start(
            plan.issue_id,
            PlanTrigger::Feedback {
                version: plan.version,
                feedback: feedback.to_string(),
            },
        )
        .await?;
        Plan::find_by_id(pool, plan.id)
            .await?
            .ok_or(PlanningError::NotFound)
    }
}

fn is_list_item(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- ")
        || line.starts_with("* ")
        || line
            .split_once(['.', ')'])
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use embeddings::service::{CodeLocation, RelatedIssue, SearchHit};

    use super::*;

    fn files(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn task_follows_the_mode() {
        let ask = planner_task("PROJ-1", "Reset mail", IssueMode::Ask, true);
        assert!(ask.contains("Mode: ask"));
        assert!(ask.contains("prior_work.md"));
        assert!(ask.contains("related.md"));
        assert!(ask.contains("/kanbain/out/plan.json"));
        let yolo = planner_task("PROJ-1", "Reset mail", IssueMode::Yolo, false);
        assert!(yolo.contains("Do not ask questions"));
        assert!(yolo.contains("`assumptions`"));
        assert!(!yolo.contains("prior_work.md"));
    }

    #[test]
    fn harvest_reads_plan_and_questions() {
        let plan = r#"{"subtasks":[{"id":1,"title":"Send mail"}],"assumptions":["SMTP"]}"#;
        let h = read_harvest(
            &files(&[
                ("plan.json", plan),
                ("questions.md", "1. Which provider?\n"),
            ]),
            None,
        );
        assert_eq!(h.plan.unwrap().unwrap().subtasks[0].id, "1");
        assert_eq!(h.questions.as_deref(), Some("1. Which provider?"));

        let h = read_harvest(&files(&[("plan.json", "not json")]), None);
        assert!(h.plan.unwrap().is_err());

        let h = read_harvest(&files(&[("questions.md", "  ")]), Some("All done."));
        assert_eq!(h, Harvest::default());
    }

    #[test]
    fn harvest_falls_back_to_a_plan_in_the_final_message() {
        let message =
            "Here is the plan:\n```json\n{\"subtasks\":[{\"id\":\"a\",\"title\":\"A\"}]}\n```";
        let h = read_harvest(&files(&[]), Some(message));
        assert_eq!(h.plan.unwrap().unwrap().subtasks[0].title, "A");
        // A JSON object that isn't a plan is ignored.
        let h = read_harvest(&files(&[]), Some("{\"ok\": true}"));
        assert!(h.plan.is_none());
    }

    fn hit(path: &str, start: i64, end: i64, symbol: &str) -> SearchHit {
        SearchHit {
            kind: "code".into(),
            score: 1.0,
            vector_rank: None,
            keyword_rank: None,
            excerpt: String::new(),
            project_id: String::new(),
            issue_id: None,
            comment_id: None,
            attachment_id: None,
            execution_process_id: None,
            repo_id: None,
            path: Some(path.into()),
            start_line: Some(start),
            end_line: Some(end),
            symbol: Some(symbol.into()),
        }
    }

    #[test]
    fn related_lists_tickets_and_code_within_the_cap() {
        let a = Uuid::new_v4();
        let related = Related {
            tickets: vec![RelatedIssue {
                issue_id: a,
                score: 1.0,
                kinds: vec!["ticket".into(), "run".into()],
                excerpt: "Password reset mails\n never arrive".into(),
                changed: vec!["fn send_reset in mailer.rs".into()],
            }],
            code: vec![CodeLocation {
                hit: hit("shop/src/mailer.rs", 10, 42, "fn send_reset"),
                changed_by: vec![a],
            }],
        };
        let info = |id: Uuid| {
            (id == a).then(|| {
                (
                    "KAN-2".to_string(),
                    "Reset mail".to_string(),
                    "Done".to_string(),
                )
            })
        };
        let md = render_related("KAN-5", &related, &info);
        assert!(md.contains("- KAN-2 \"Reset mail\" (Done)"), "{md}");
        assert!(md.contains("Changed: fn send_reset in mailer.rs"));
        assert!(md.contains("> Password reset mails never arrive"));
        assert!(md.contains("- shop/src/mailer.rs:10-42 `fn send_reset` (changed by KAN-2)"));

        let many = Related {
            tickets: vec![],
            code: (0..500)
                .map(|i| CodeLocation {
                    hit: hit(
                        &format!("repo/some/fairly/long/path/file_{i}.rs"),
                        1,
                        9,
                        "fn x",
                    ),
                    changed_by: vec![],
                })
                .collect(),
        };
        let md = render_related("KAN-5", &many, &info);
        assert!(md.len() <= RELATED_MAX_BYTES);
        assert_eq!(md.matches("\n- repo/").count(), RELATED_CODE);

        // A ticket changing several hunks is named once; unknown tickets
        // (other projects) not at all.
        let dup = Related {
            tickets: vec![],
            code: vec![CodeLocation {
                hit: hit("shop/src/mailer.rs", 1, 3, "fn send_reset"),
                changed_by: vec![a, a, Uuid::new_v4()],
            }],
        };
        let md = render_related("KAN-5", &dup, &info);
        assert!(md.contains("`fn send_reset` (changed by KAN-2)\n"), "{md}");

        let empty = Related {
            tickets: vec![],
            code: vec![],
        };
        assert!(render_related("KAN-5", &empty, &info).contains("Nothing related"));
    }

    #[test]
    fn counts_list_items() {
        assert!(is_list_item("1. Which?"));
        assert!(is_list_item("  - Which?"));
        assert!(is_list_item("2) Which?"));
        assert!(!is_list_item("Version 1.2 is out"));
    }
}
