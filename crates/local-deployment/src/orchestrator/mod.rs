//! The orchestrator's runtime (Phase 5, D22): carries out an approved plan.
//!
//! Every subtask becomes a sub-ticket and gets its own workspace on
//! `<ticket branch>--<id>`, branched from the ticket branch, with a coder run
//! in its own sandbox container. The scheduling decisions (who may start,
//! when the ticket is done or blocked) come from
//! [`services::services::orchestrator::Schedule`]; this module starts runs,
//! merges finished subtask branches into the ticket branch on the host, and
//! turns a merge conflict into a re-run of that subtask on top of the
//! updated ticket branch. All state is in SQLite, so a restart resumes.
//! From Phase 6 the plan's checkpoints are tested and fixed along the way
//! (see [`checkpoints`]); a ticket reaches Review only once they all pass.
//!
//! Locking: one async mutex per ticket guards its rows and the ticket
//! worktree (merges are serialized). It is never held across
//! `start_execution`, which can wait minutes for a sandbox slot, nor across
//! `stop_execution`, which waits for the run's exit monitor, which in turn
//! calls back into [`OrchestratorService::on_run_finished`].

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use db::{
    DBService,
    models::{
        execution_process::{ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus},
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::{CreateIssue, Issue},
        issue_event::{IssueEvent, IssueEventType},
        issue_found::{IssueFound, IssueFoundStatus},
        orchestration::{
            CheckpointStatus, OrchestrationCheckpoint, OrchestrationStatus, OrchestrationSubtask,
            SubtaskConflict, SubtaskStatus, TicketOrchestration,
        },
        plan::{Plan, PlanDocument, PlanStatus, PlanSubtask},
        project::Project,
        project_status::{ProjectStatus, ProjectStatusKind},
        requests::WorkspaceRepoInput,
        session::{CreateSession, Session},
        workspace::{CreateWorkspace, Workspace},
        workspace_repo::WorkspaceRepo,
    },
};
use embeddings::{
    SearchService,
    service::{Related, SearchRequest},
    store::KIND_CODE,
};
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType, coding_agent_initial::CodingAgentInitialRequest,
    },
    adapters::{AgentRole, executor_for},
    model_selector::PermissionPolicy,
    profile::ExecutorConfig,
};
use git::{GitService, GitServiceError};
use serde::Serialize;
use serde_json::json;
use services::services::{
    agent_library::AgentLibrary,
    container::ContainerService,
    orchestrator::{Outcome, Schedule},
};
use tokio::sync::Mutex;
use ts_rs::TS;
use utils::ticket_id::subtask_branch_name;
use uuid::Uuid;
use workspace_manager::WorkspaceManager;

use crate::{
    container::LocalContainerService,
    jobs::AgentJobs,
    planning::{PlanningService, move_to_kind, render_related},
};

mod checkpoints;
pub use checkpoints::{CheckpointView, test_path_matcher};

/// Fix rounds per ticket before it goes to Blocked (D24): merge-conflict
/// re-runs and fix runs for the tester's issues count alike.
pub const MAX_FIX_ROUNDS: i64 = 3;
/// Cap on the earlier attempt's diff a conflict re-run gets.
const PREVIOUS_DIFF_MAX_BYTES: usize = 64 * 1024;
/// Code locations in a coder's `related.md`.
const RELATED_CODE: usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
}

/// How merging a run's branch into the ticket branch went, over all repos.
pub(crate) enum MergeOutcome {
    /// Merge commit per repo (none for repos without new commits).
    Merged(BTreeMap<String, String>),
    /// Conflicted files per repo; every merge was aborted.
    Conflicts(Vec<(String, Vec<String>)>),
    Failed(String),
}

impl From<crate::planning::PlanningError> for OrchestratorError {
    fn from(e: crate::planning::PlanningError) -> Self {
        match e {
            crate::planning::PlanningError::Database(e) => Self::Database(e),
            crate::planning::PlanningError::NotFound => Self::NotFound,
            other => Self::Invalid(other.to_string()),
        }
    }
}

/// One subtask as the ticket panel shows it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SubtaskView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub subtask: OrchestrationSubtask,
    pub title: String,
    pub files: Vec<String>,
    /// Plan ids this subtask waits for: its dependencies plus earlier
    /// subtasks with overlapping files.
    pub waits_for: Vec<String>,
    pub branch: Option<String>,
    /// The sub-ticket's display id, e.g. `KAN-43`.
    pub sub_issue: Option<String>,
    /// Model cost of every run in the subtask's workspace.
    pub cost_usd: f64,
}

/// What the ticket panel shows about coding.
#[derive(Debug, Clone, Serialize, TS)]
pub struct OrchestrationView {
    pub orchestration: Option<TicketOrchestration>,
    /// The ticket branch subtasks merge into.
    pub ticket_branch: Option<String>,
    pub subtasks: Vec<SubtaskView>,
    /// The checkpoints and their tester and fix runs (Phase 6).
    pub checkpoints: Vec<CheckpointView>,
    /// What the tester found, oldest first.
    pub issues_found: Vec<IssueFound>,
}

/// The first message of a coder run.
pub fn coder_task(id: &str, subtask: &PlanSubtask, n: usize, total: usize, rerun: bool) -> String {
    let mut task = format!(
        "Implement subtask {} of ticket {id}: \"{}\" ({n} of {total}).\n\n\
         Read these first (read-only):\n\
         - /kanbain/context/subtask.md: this subtask, its files and what runs next to it.\n\
         - /kanbain/context/ticket.md: the ticket.\n\
         - /kanbain/context/plan.json: the approved plan, for context only.\n\
         - /kanbain/context/related.md: code locations search found for this subtask. \
         Check it before exploring the code.\n",
        subtask.id, subtask.title
    );
    if rerun {
        task.push_str(
            "- /kanbain/context/conflict.md: why you are running again. Read it before \
             anything else.\n",
        );
    }
    task.push_str(&format!(
        "\nRules:\n\
         - Do only this subtask. Other subtasks run in parallel on their own branches; \
         stay out of files outside this subtask unless the change really needs them.\n\
         - Commit your work in the repository under /workspace with messages starting \
         `{id}: `. Leave nothing uncommitted.\n\
         - Do not create or switch branches, merge, rebase or push. KanbAIn merges your \
         branch into the ticket branch when you finish.\n"
    ));
    task
}

/// `subtask.md` for a coder run.
pub fn render_subtask(
    plan: &PlanDocument,
    index: usize,
    schedule: &Schedule,
    statuses: &[SubtaskStatus],
) -> String {
    let s = &plan.subtasks[index];
    let mut out = format!(
        "# Subtask {} of {}: {}\n\n{}\n",
        s.id,
        plan.subtasks.len(),
        s.title,
        s.description.trim()
    );
    if !s.files.is_empty() {
        out.push_str("\n## Files it likely touches\n\n");
        for f in &s.files {
            out.push_str(&format!("- {f}\n"));
        }
    }
    let line = |i: usize| format!("- {}: {}\n", plan.subtasks[i].id, plan.subtasks[i].title);
    let before: Vec<usize> = schedule.before(index).collect();
    if !before.is_empty() {
        out.push_str("\n## Already merged into your branch\n\n");
        for &i in &before {
            out.push_str(&line(i));
        }
    }
    let parallel: Vec<usize> = (0..plan.subtasks.len())
        .filter(|&i| i != index && statuses[i] == SubtaskStatus::Running)
        .collect();
    if !parallel.is_empty() {
        out.push_str("\n## Running in parallel (leave their files alone)\n\n");
        for &i in &parallel {
            let other = &plan.subtasks[i];
            out.push_str(&format!("- {}: {}", other.id, other.title));
            if !other.files.is_empty() {
                out.push_str(&format!(" ({})", other.files.join(", ")));
            }
            out.push('\n');
        }
    }
    if !plan.acceptance_criteria.is_empty() {
        out.push_str("\n## The ticket's acceptance criteria\n\n");
        for c in &plan.acceptance_criteria {
            out.push_str(&format!("- {c}\n"));
        }
    }
    out
}

/// `conflict.md` for a re-run after a merge conflict (D22).
pub fn render_conflict(conflict: &SubtaskConflict, ticket_branch: &str, round: i64) -> String {
    let mut out = format!(
        "# Merge conflict (fix round {round} of {MAX_FIX_ROUNDS})\n\n\
         Your earlier attempt at this subtask could not be merged into the ticket branch \
         `{ticket_branch}`: work merged there meanwhile changed the same lines.\n\n\
         Conflicted files:\n"
    );
    for f in &conflict.files {
        out.push_str(&format!("- {f}\n"));
    }
    out.push_str(
        "\nYour branch now starts from the updated ticket branch, without your earlier \
         commits. Your earlier changes are in /kanbain/context/previous_attempt.diff \
         (paths are relative to each repository). Re-apply them on top of the current \
         code, for example with `git apply --3way /kanbain/context/previous_attempt.diff` \
         inside the repository, keep what the other subtasks merged, resolve the \
         conflicts, check the result, and commit.\n",
    );
    out
}

#[derive(Clone)]
pub struct OrchestratorService {
    db: DBService,
    container: LocalContainerService,
    workspace_manager: WorkspaceManager,
    git: GitService,
    library: Arc<AgentLibrary>,
    jobs: AgentJobs,
    planning: PlanningService,
    search: Arc<OnceLock<SearchService>>,
    locks: Arc<std::sync::Mutex<HashMap<Uuid, Arc<Mutex<()>>>>>,
    /// Set when the app shuts down and kills every run.
    shutting_down: Arc<AtomicBool>,
}

impl OrchestratorService {
    pub fn new(
        db: DBService,
        container: LocalContainerService,
        workspace_manager: WorkspaceManager,
        git: GitService,
        library: Arc<AgentLibrary>,
        jobs: AgentJobs,
        planning: PlanningService,
    ) -> Self {
        Self {
            db,
            container,
            workspace_manager,
            git,
            library,
            jobs,
            planning,
            search: Arc::default(),
            locks: Arc::default(),
            shutting_down: Arc::default(),
        }
    }

    pub fn set_search(&self, search: SearchService) {
        let _ = self.search.set(search);
    }

    /// The app is about to kill every run. Coder runs that die from it stay
    /// `running` in SQLite, so the next launch requeues and resumes them
    /// instead of blocking the ticket over a "failed" run; nothing new
    /// starts meanwhile.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    fn lock_for(&self, issue_id: Uuid) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(issue_id)
            .or_default()
            .clone()
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
            tracing::warn!("Recording an orchestrator event for {issue_id} failed: {e}");
        }
    }

    async fn move_issue(&self, issue_id: Option<Uuid>, kind: ProjectStatusKind) {
        let Some(issue_id) = issue_id else { return };
        if let Ok(Some(issue)) = Issue::find_by_id(&self.db.pool, issue_id).await {
            move_to_kind(&self.db.pool, &issue, kind).await;
        }
    }

    async fn notify(&self, issue_id: Uuid, title: &str, message: &str) {
        // Logged too: on Linux (and in the demos) the OS notification is
        // invisible, and "did it fire?" is the question you have later.
        tracing::info!(%issue_id, "Notification: {title}: {message}");
        let workspace = TicketOrchestration::find(&self.db.pool, issue_id)
            .await
            .ok()
            .flatten()
            .and_then(|o| o.workspace_id);
        self.container
            .notification_service()
            .notify(title, message, workspace)
            .await;
    }

    async fn display_id(&self, issue: &Issue) -> String {
        let prefix = match Project::find_by_id(&self.db.pool, issue.project_id).await {
            Ok(Some(p)) => p.id_prefix,
            _ => "KAN".to_string(),
        };
        issue.display_id(&prefix)
    }

    // ---- Views -------------------------------------------------------------

    pub async fn view(&self, issue_id: Uuid) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let orchestration = TicketOrchestration::find(pool, issue_id).await?;
        let Some(o) = &orchestration else {
            return Ok(OrchestrationView {
                orchestration,
                ticket_branch: None,
                subtasks: vec![],
                checkpoints: vec![],
                issues_found: vec![],
            });
        };
        let ticket_branch = match o.workspace_id {
            Some(id) => Workspace::find_by_id(pool, id).await?.map(|w| w.branch),
            None => None,
        };
        let plan = Plan::find_by_id(pool, o.plan_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let schedule = Schedule::new(&plan.plan.0);
        let prefix = match Issue::find_by_id(pool, issue_id).await? {
            Some(issue) => Project::find_by_id(pool, issue.project_id)
                .await?
                .map(|p| p.id_prefix)
                .unwrap_or_else(|| "KAN".into()),
            None => "KAN".into(),
        };
        let mut subtasks = Vec::new();
        for row in OrchestrationSubtask::find_by_issue(pool, issue_id).await? {
            let index = plan
                .plan
                .0
                .subtasks
                .iter()
                .position(|s| s.id == row.plan_subtask_id);
            let (title, files, waits_for) = match index {
                Some(i) => {
                    let s = &plan.plan.0.subtasks[i];
                    (
                        s.title.clone(),
                        s.files.clone(),
                        schedule
                            .before(i)
                            .map(|d| plan.plan.0.subtasks[d].id.clone())
                            .collect(),
                    )
                }
                None => (row.plan_subtask_id.clone(), vec![], vec![]),
            };
            let branch = match row.workspace_id {
                Some(id) => Workspace::find_by_id(pool, id).await?.map(|w| w.branch),
                None => None,
            };
            let sub_issue = match row.sub_issue_id {
                Some(id) => Issue::find_by_id(pool, id)
                    .await?
                    .map(|i| i.display_id(&prefix)),
                None => None,
            };
            let cost_usd = match row.workspace_id {
                Some(id) => workspace_cost(pool, id).await?,
                None => 0.0,
            };
            subtasks.push(SubtaskView {
                subtask: row,
                title,
                files,
                waits_for,
                branch,
                sub_issue,
                cost_usd,
            });
        }
        let checkpoints = self.checkpoint_views(issue_id, &plan.plan.0).await?;
        let issues_found = IssueFound::find_by_issue(pool, issue_id).await?;
        Ok(OrchestrationView {
            orchestration,
            ticket_branch,
            subtasks,
            checkpoints,
            issues_found,
        })
    }

    // ---- Starting ----------------------------------------------------------

    /// Starts coding the ticket's approved plan: sub-tickets, subtask rows,
    /// then the first scheduling step. Starting again with the same plan
    /// retries what failed or was stopped.
    pub async fn start(&self, issue_id: Uuid) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let plan = Plan::latest(pool, issue_id)
            .await?
            .filter(|p| p.status == PlanStatus::Approved)
            .ok_or_else(|| OrchestratorError::Invalid("Approve a plan first".into()))?;
        if !self.container.sandbox().enabled().await {
            return Err(OrchestratorError::Invalid(
                "Coders run in the sandbox; turn it on in Settings → Sandbox".into(),
            ));
        }
        for role in [AgentRole::Coder, AgentRole::Tester] {
            self.jobs
                .agent_name_for(role)
                .await
                .map_err(OrchestratorError::Invalid)?;
        }

        if let Some(existing) = TicketOrchestration::find(pool, issue_id).await?
            && existing.plan_id == plan.id
        {
            return self.retry_ticket(issue_id).await;
        }

        let lock = self.lock_for(issue_id);
        let guard = lock.lock().await;
        if let Some(existing) = TicketOrchestration::find(pool, issue_id).await? {
            if existing.status == OrchestrationStatus::Running {
                return Err(OrchestratorError::Invalid(
                    "Stop the running coders of the earlier plan first".into(),
                ));
            }
            // A new plan replaces the old one's unfinished subtasks; merged
            // work stays on the ticket branch and its sub-tickets stay Done.
            for old in OrchestrationSubtask::find_by_issue(pool, issue_id).await? {
                if old.status != SubtaskStatus::Merged
                    && let Some(sub) = old.sub_issue_id
                {
                    let _ = Issue::delete(pool, sub).await;
                }
            }
            OrchestrationSubtask::delete_by_issue(pool, issue_id).await?;
            // The old plan's checkpoints and what they found go too; the
            // scheduling step creates the new plan's checkpoints.
            OrchestrationCheckpoint::delete_by_issue(pool, issue_id).await?;
            IssueFound::delete_by_issue(pool, issue_id).await?;
        }

        let workspace = self.planning.ticket_workspace(&issue).await?;
        TicketOrchestration::upsert(pool, issue_id, plan.id, workspace.id).await?;
        let backlog =
            ProjectStatus::find_first_of_kind(pool, issue.project_id, ProjectStatusKind::Backlog)
                .await?
                .map(|s| s.id);
        for (position, subtask) in plan.plan.0.subtasks.iter().enumerate() {
            let mut description = subtask.description.trim().to_string();
            if !subtask.files.is_empty() {
                description.push_str(&format!("\n\nFiles: {}", subtask.files.join(", ")));
            }
            let sub_issue = Issue::create(
                pool,
                issue.project_id,
                &CreateIssue {
                    title: subtask.title.clone(),
                    description: Some(description),
                    status_id: backlog,
                    priority: issue.priority,
                    sort_order: None,
                    parent_issue_id: Some(issue.id),
                    parent_issue_sort_order: Some(position as f64),
                    external_id: None,
                    mode: Some(issue.mode),
                    budget_usd: None,
                },
            )
            .await
            .map_err(|e| OrchestratorError::Invalid(format!("Creating a sub-ticket: {e}")))?;
            OrchestrationSubtask::create(
                pool,
                issue_id,
                &subtask.id,
                position as i64,
                Some(sub_issue.id),
            )
            .await?;
        }
        Issue::set_fix_round(pool, issue_id, 0).await?;
        self.event(
            issue_id,
            IssueEventType::CodingStarted,
            "system",
            json!({"version": plan.version, "subtasks": plan.plan.0.subtasks.len(),
                   "branch": workspace.branch}),
        )
        .await;
        move_to_kind(pool, &issue, ProjectStatusKind::InProgress).await;
        self.tick_locked(issue_id).await?;
        drop(guard);
        self.view(issue_id).await
    }

    /// Picks up running tickets after an app restart. Their interrupted
    /// runs were set back to pending at startup.
    pub async fn resume(&self) {
        let pool = &self.db.pool;
        let issues = match TicketOrchestration::find_running(pool).await {
            Ok(issues) => issues,
            Err(e) => {
                tracing::warn!("Listing running orchestrations failed: {e}");
                return;
            }
        };
        for issue_id in issues {
            self.event(
                issue_id,
                IssueEventType::CodingStarted,
                "system",
                json!({"reason": "restart"}),
            )
            .await;
            if let Err(e) = self.tick(issue_id).await {
                tracing::warn!("Resuming the orchestration of {issue_id} failed: {e}");
            }
            // A tester or fix run the restart interrupted starts that step
            // again; pending checkpoints are the scheduling step's job.
            let checkpoints = match OrchestrationCheckpoint::find_by_issue(pool, issue_id).await {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!("Listing the checkpoints of {issue_id} failed: {e}");
                    continue;
                }
            };
            for row in checkpoints {
                if matches!(
                    row.status,
                    CheckpointStatus::Testing | CheckpointStatus::Fixing
                ) && row.execution_process_id.is_none()
                {
                    let service = self.clone();
                    tokio::spawn(async move { service.launch_check(row.id).await });
                }
            }
        }
    }

    pub async fn tick(&self, issue_id: Uuid) -> Result<(), OrchestratorError> {
        let lock = self.lock_for(issue_id);
        let _guard = lock.lock().await;
        self.tick_locked(issue_id).await
    }

    /// One scheduling step: start what may start, then settle the ticket
    /// when nothing can move any more. Caller holds the ticket lock.
    async fn tick_locked(&self, issue_id: Uuid) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let Some(o) = TicketOrchestration::find(pool, issue_id).await? else {
            return Ok(());
        };
        if o.status != OrchestrationStatus::Running || self.is_shutting_down() {
            return Ok(());
        }
        let plan = Plan::find_by_id(pool, o.plan_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let doc = &plan.plan.0;
        let rows = OrchestrationSubtask::find_by_issue(pool, issue_id).await?;
        let by_plan_id: HashMap<&str, &OrchestrationSubtask> = rows
            .iter()
            .map(|r| (r.plan_subtask_id.as_str(), r))
            .collect();
        let Some(ordered) = doc
            .subtasks
            .iter()
            .map(|s| by_plan_id.get(s.id.as_str()).copied())
            .collect::<Option<Vec<_>>>()
        else {
            TicketOrchestration::set_status(
                pool,
                issue_id,
                OrchestrationStatus::Blocked,
                Some("The subtasks don't match the approved plan; start coding again"),
            )
            .await?;
            return Ok(());
        };
        let mut statuses: Vec<SubtaskStatus> = ordered.iter().map(|r| r.status).collect();
        let schedule = Schedule::new(doc);
        let check_rows = self.ensure_checkpoints(issue_id, doc, &schedule).await?;
        let mut checks: Vec<CheckpointStatus> = check_rows.iter().map(|r| r.status).collect();
        let step = schedule.step(&statuses, &checks);

        for i in step.subtasks {
            let row = ordered[i];
            if !OrchestrationSubtask::claim(pool, row.id).await? {
                continue;
            }
            statuses[i] = SubtaskStatus::Running;
            self.move_issue(row.sub_issue_id, ProjectStatusKind::InProgress)
                .await;
            self.event(
                issue_id,
                IssueEventType::SubtaskStarted,
                "system",
                json!({"subtask": doc.subtasks[i].id, "title": doc.subtasks[i].title,
                       "fix_round": row.fix_round}),
            )
            .await;
            let service = self.clone();
            let subtask_id = row.id;
            tokio::spawn(async move { service.launch(subtask_id).await });
        }
        if let Some(k) = step.checkpoint
            && let Some(row) = check_rows.get(k)
            && OrchestrationCheckpoint::claim(pool, row.id).await?
        {
            checks[k] = CheckpointStatus::Testing;
            self.event(
                issue_id,
                IssueEventType::TesterStarted,
                "system",
                json!({"checkpoint": row.position + 1, "what": row.what, "round": row.round}),
            )
            .await;
            let service = self.clone();
            let checkpoint_id = row.id;
            tokio::spawn(async move { service.launch_check(checkpoint_id).await });
        }

        let (status, error, event_status) = match schedule.step(&statuses, &checks).outcome {
            Outcome::Continue => {
                // The ticket sits in Testing while a checkpoint is tested or
                // fixed, in In progress while only coders run.
                let testing = checks
                    .iter()
                    .any(|c| matches!(c, CheckpointStatus::Testing | CheckpointStatus::Fixing));
                let kind = if testing {
                    ProjectStatusKind::Testing
                } else {
                    ProjectStatusKind::InProgress
                };
                self.move_issue(Some(issue_id), kind).await;
                return Ok(());
            }
            Outcome::Done => (OrchestrationStatus::Done, None, "done"),
            Outcome::Blocked => {
                let mut failed: Vec<String> = ordered
                    .iter()
                    .zip(&doc.subtasks)
                    .filter(|(r, _)| r.status == SubtaskStatus::Failed)
                    .map(|(r, s)| match &r.error {
                        Some(e) => format!("{} ({e})", s.title),
                        None => s.title.clone(),
                    })
                    .collect();
                // The rows as they are now: a failure sets the error.
                for r in OrchestrationCheckpoint::find_by_issue(pool, issue_id).await? {
                    if r.status == CheckpointStatus::Failed {
                        failed.push(match &r.error {
                            Some(e) => format!("checkpoint {} ({e})", r.position + 1),
                            None => format!("checkpoint {}", r.position + 1),
                        });
                    }
                }
                (
                    OrchestrationStatus::Blocked,
                    Some(format!("Failed: {}", failed.join("; "))),
                    "blocked",
                )
            }
            Outcome::Stopped => (OrchestrationStatus::Stopped, None, "stopped"),
        };
        TicketOrchestration::set_status(pool, issue_id, status, error.as_deref()).await?;
        self.event(
            issue_id,
            IssueEventType::CodingFinished,
            "system",
            json!({"status": event_status, "error": error}),
        )
        .await;
        let Some(issue) = Issue::find_by_id(pool, issue_id).await? else {
            return Ok(());
        };
        match status {
            OrchestrationStatus::Done => {
                move_to_kind(pool, &issue, ProjectStatusKind::Review).await;
                self.notify(
                    issue_id,
                    "Ready for review",
                    &format!("All subtasks of \"{}\" are merged and tested.", issue.title),
                )
                .await;
            }
            OrchestrationStatus::Blocked => {
                move_to_kind(pool, &issue, ProjectStatusKind::Blocked).await;
                self.notify(
                    issue_id,
                    "Ticket blocked",
                    &format!(
                        "\"{}\": {}",
                        issue.title,
                        error.as_deref().unwrap_or("a subtask failed")
                    ),
                )
                .await;
            }
            _ => {}
        }
        Ok(())
    }

    // ---- Launching a coder -------------------------------------------------

    /// Boxed: a launch ends in a scheduling step, which spawns launches;
    /// without the box that cycle has no nameable (or provably `Send`) type.
    fn launch(
        &self,
        subtask_id: Uuid,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.launch_unboxed(subtask_id))
    }

    async fn launch_unboxed(&self, subtask_id: Uuid) {
        let Err(e) = self.launch_inner(subtask_id).await else {
            return;
        };
        if self.is_shutting_down() {
            return;
        }
        tracing::warn!("Starting the coder for subtask {subtask_id} failed: {e}");
        let Ok(Some(row)) = OrchestrationSubtask::find_by_id(&self.db.pool, subtask_id).await
        else {
            return;
        };
        let lock = self.lock_for(row.issue_id);
        let _guard = lock.lock().await;
        // Stopped while it was being prepared: that's not a failure.
        let Ok(Some(row)) = OrchestrationSubtask::find_by_id(&self.db.pool, subtask_id).await
        else {
            return;
        };
        if row.status != SubtaskStatus::Running {
            let _ = self.tick_locked(row.issue_id).await;
            return;
        }
        if let Err(err) = self.fail_locked(&row, &e.to_string()).await {
            tracing::warn!("Recording the failure of subtask {subtask_id} failed: {err}");
        }
        let _ = self.tick_locked(row.issue_id).await;
    }

    async fn launch_inner(&self, subtask_id: Uuid) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationSubtask::find_by_id(pool, subtask_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let plan = Plan::find_by_id(pool, o.plan_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let doc = &plan.plan.0;
        let index = doc
            .subtasks
            .iter()
            .position(|s| s.id == row.plan_subtask_id)
            .ok_or(OrchestratorError::NotFound)?;
        let ticket_ws = self.ticket_workspace(&o).await?;
        let workspace = self
            .subtask_workspace(&issue, &ticket_ws, &row, &doc.subtasks[index])
            .await?;

        let agent_name = self
            .jobs
            .agent_name_for(AgentRole::Coder)
            .await
            .map_err(OrchestratorError::Invalid)?;
        let agent = self
            .library
            .get_agent(&agent_name)
            .await
            .map_err(|e| OrchestratorError::Invalid(format!("Agent `{agent_name}`: {e}")))?;
        let mut executor_config = ExecutorConfig::new(executor_for(agent.runtime));
        executor_config.agent_definition = Some(agent.name.clone());
        executor_config.permission_policy = Some(PermissionPolicy::Auto);
        // A fresh session every time: the container (and with it the
        // runtime's session files) is gone after each run (D37), and a
        // conflict re-run gets what it needs from conflict.md.
        let session = Session::create(
            pool,
            &CreateSession {
                executor: Some(executor_config.executor.to_string()),
                name: Some(format!("Coder · subtask {}", row.plan_subtask_id)),
            },
            Uuid::new_v4(),
            workspace.id,
        )
        .await
        .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        let id = self.display_id(&issue).await;
        let prompt = coder_task(
            &id,
            &doc.subtasks[index],
            index + 1,
            doc.subtasks.len(),
            row.conflict.is_some(),
        );
        // No setup script chained: it would hold the subtask's slot without
        // the agent, and coders install what they need themselves.
        let action = ExecutorAction::new(
            ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                prompt,
                executor_config,
                working_dir: None,
            }),
            None,
        );
        let execution = self
            .container
            .start_execution(
                &workspace,
                &session,
                &action,
                &ExecutionProcessRunReason::CodingAgent,
            )
            .await
            .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;

        let stop = {
            let lock = self.lock_for(row.issue_id);
            let _guard = lock.lock().await;
            let current = OrchestrationSubtask::find_by_id(pool, subtask_id).await?;
            if current
                .as_ref()
                .is_some_and(|c| c.execution_process_id.is_none())
            {
                OrchestrationSubtask::set_run(pool, subtask_id, execution.id).await?;
            }
            if let Some(sub) = row.sub_issue_id {
                ExecutionProcess::set_subtask(pool, execution.id, sub).await?;
            }
            // Stopped while it waited for a sandbox slot.
            current.is_some_and(|c| c.status == SubtaskStatus::Stopped)
        };
        if stop {
            self.stop_run(execution.id).await;
        }
        Ok(())
    }

    /// The workspace holding the ticket branch, its worktree recreated if
    /// the periodic cleanup removed it.
    async fn ticket_workspace(
        &self,
        o: &TicketOrchestration,
    ) -> Result<Workspace, OrchestratorError> {
        let pool = &self.db.pool;
        let workspace = match o.workspace_id {
            Some(id) => Workspace::find_by_id(pool, id).await?,
            None => None,
        }
        .ok_or_else(|| {
            OrchestratorError::Invalid("The ticket's workspace was deleted".to_string())
        })?;
        self.container
            .ensure_container_exists(&workspace)
            .await
            .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        Ok(Workspace::find_by_id(pool, workspace.id)
            .await?
            .unwrap_or(workspace))
    }

    /// The subtask's own workspace on `<ticket branch>--<id>`, branched from
    /// the ticket branch in every repo of the ticket's workspace.
    async fn subtask_workspace(
        &self,
        issue: &Issue,
        ticket_ws: &Workspace,
        row: &OrchestrationSubtask,
        subtask: &PlanSubtask,
    ) -> Result<Workspace, OrchestratorError> {
        let pool = &self.db.pool;
        if let Some(id) = row.workspace_id
            && let Some(workspace) = Workspace::find_by_id(pool, id).await?
        {
            self.container
                .ensure_container_exists(&workspace)
                .await
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
            return Ok(Workspace::find_by_id(pool, id).await?.unwrap_or(workspace));
        }
        let base = subtask_branch_name(&ticket_ws.branch, &subtask.id);
        let mut branch = base.clone();
        let mut n = 2;
        while Workspace::branch_in_use(pool, &branch).await? {
            branch = format!("{base}-{n}");
            n += 1;
        }
        let id = self.display_id(issue).await;
        let workspace = Workspace::create(
            pool,
            &CreateWorkspace {
                branch,
                name: Some(format!("{id} · {}. {}", subtask.id, subtask.title)),
                issue_id: Some(issue.id),
            },
            Uuid::new_v4(),
        )
        .await
        .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        let mut managed = self
            .workspace_manager
            .load_managed_workspace(workspace)
            .await?;
        for repo in WorkspaceRepo::find_repos_for_workspace(pool, ticket_ws.id).await? {
            managed
                .add_repository(
                    &WorkspaceRepoInput {
                        repo_id: repo.id,
                        target_branch: ticket_ws.branch.clone(),
                    },
                    &self.git,
                )
                .await
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        let workspace = managed.workspace.clone();
        self.container
            .create(&workspace)
            .await
            .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        OrchestrationSubtask::set_workspace(pool, row.id, workspace.id).await?;
        Ok(Workspace::find_by_id(pool, workspace.id)
            .await?
            .unwrap_or(workspace))
    }

    /// `/kanbain/context` files for a coder run in a subtask's workspace:
    /// `subtask.md`, `plan.json`, `related.md` and, after a conflict,
    /// `conflict.md` with `previous_attempt.diff`.
    pub async fn context_files(&self, workspace_id: Uuid) -> Vec<(String, String)> {
        match self.context_files_inner(workspace_id).await {
            Ok(files) => files,
            Err(e) => {
                tracing::warn!("Coder context for workspace {workspace_id} failed: {e}");
                Vec::new()
            }
        }
    }

    async fn context_files_inner(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<(String, String)>, OrchestratorError> {
        let pool = &self.db.pool;
        if let Some(check) = OrchestrationCheckpoint::find_by_workspace(pool, workspace_id).await? {
            return self.check_context_files(&check, workspace_id).await;
        }
        let Some(row) = OrchestrationSubtask::find_by_workspace(pool, workspace_id).await? else {
            return Ok(Vec::new());
        };
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let plan = Plan::find_by_id(pool, o.plan_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let doc = &plan.plan.0;
        let Some(index) = doc
            .subtasks
            .iter()
            .position(|s| s.id == row.plan_subtask_id)
        else {
            return Ok(Vec::new());
        };
        let rows = OrchestrationSubtask::find_by_issue(pool, row.issue_id).await?;
        let statuses: Vec<SubtaskStatus> = doc
            .subtasks
            .iter()
            .map(|s| {
                rows.iter()
                    .find(|r| r.plan_subtask_id == s.id)
                    .map_or(SubtaskStatus::Pending, |r| r.status)
            })
            .collect();
        let schedule = Schedule::new(doc);
        let id = self.display_id(&issue).await;

        let mut files = vec![
            (
                "subtask.md".to_string(),
                render_subtask(doc, index, &schedule, &statuses),
            ),
            (
                "plan.json".to_string(),
                serde_json::to_string_pretty(doc).unwrap_or_default(),
            ),
            (
                "related.md".to_string(),
                self.related_code(&issue, &id, &doc.subtasks[index]).await,
            ),
        ];
        if let Some(conflict) = &row.conflict {
            let ticket_branch = match o.workspace_id {
                Some(id) => Workspace::find_by_id(pool, id).await?.map(|w| w.branch),
                None => None,
            }
            .unwrap_or_default();
            files.push((
                "conflict.md".to_string(),
                render_conflict(&conflict.0, &ticket_branch, row.fix_round),
            ));
            files.push((
                "previous_attempt.diff".to_string(),
                conflict.0.previous_diff.clone(),
            ));
        }
        Ok(files)
    }

    /// Code hits for the subtask (Phase 4 leftover: coders get the code
    /// locations of their subtask's file list), as `related.md`.
    async fn related_code(&self, issue: &Issue, id: &str, subtask: &PlanSubtask) -> String {
        let Some(search) = self.search.get() else {
            return format!("# Related work for {id}\n\nSearch is not available.\n");
        };
        let query = format!(
            "{}\n{}\n{}",
            subtask.title,
            subtask.description,
            subtask.files.join("\n")
        );
        let hits = match search
            .search(&SearchRequest {
                query,
                project_id: Some(issue.project_id),
                issue_id: Some(issue.id),
                kinds: Some(vec![KIND_CODE.to_string()]),
                limit: Some(RELATED_CODE as u32),
                ..Default::default()
            })
            .await
        {
            Ok(response) => response.hits,
            Err(e) => {
                tracing::warn!("Code search for {id} failed: {e}");
                return format!("# Related work for {id}\n\nSearch failed: {e}\n");
            }
        };
        let mut code = Vec::new();
        for hit in hits {
            match search.code_location(hit).await {
                Ok(location) => code.push(location),
                Err(e) => tracing::warn!("Code location for {id} failed: {e}"),
            }
        }
        render_related(
            id,
            &Related {
                tickets: vec![],
                code,
            },
            &|_| None,
        )
    }

    // ---- When a coder run ends ---------------------------------------------

    /// Called when any coding run ends. If it was a subtask's coder, its
    /// branch is merged into the ticket branch (or re-run on a conflict) and
    /// the next subtasks start.
    pub async fn on_run_finished(&self, execution_id: Uuid) {
        // Nothing else ever looks at a finished run again, so a lost end
        // leaves its subtask or checkpoint stuck. A busy database is worth
        // a few more tries; the handling is safe to repeat (a merged branch
        // merges as nothing, a squashed one squashes to itself).
        for attempt in 1..=4 {
            match self.finish_run(execution_id).await {
                Ok(()) => return,
                Err(OrchestratorError::Database(e)) if attempt < 4 => {
                    tracing::warn!(
                        "Handling the end of run {execution_id} failed (try {attempt}): {e}"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
                }
                Err(e) => {
                    tracing::warn!("Handling the end of run {execution_id} failed: {e}");
                    return;
                }
            }
        }
    }

    async fn finish_run(&self, execution_id: Uuid) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let Some(process) = ExecutionProcess::find_by_id(pool, execution_id).await? else {
            return Ok(());
        };
        if process.run_reason != ExecutionProcessRunReason::CodingAgent {
            return Ok(());
        }
        // Killed by the shutdown: leave it running for the next launch. A
        // run that completed just before still merges.
        if self.is_shutting_down() && process.status != ExecutionProcessStatus::Completed {
            return Ok(());
        }
        let Some(session) = Session::find_by_id(pool, process.session_id).await? else {
            return Ok(());
        };
        // A checkpoint's tester or fix run.
        if let Some(check) =
            OrchestrationCheckpoint::find_by_workspace(pool, session.workspace_id).await?
        {
            let lock = self.lock_for(check.issue_id);
            let _guard = lock.lock().await;
            let check = OrchestrationCheckpoint::find_by_id(pool, check.id)
                .await?
                .ok_or(OrchestratorError::NotFound)?;
            // A manual follow-up in the checkpoint's workspace is not its run.
            if check
                .execution_process_id
                .is_some_and(|id| id != execution_id)
            {
                return Ok(());
            }
            return self
                .finish_check_locked(&check, &process, session.workspace_id)
                .await;
        }
        // By workspace, not by run id: a quick run can end before its id is
        // recorded on the subtask.
        let Some(row) = OrchestrationSubtask::find_by_workspace(pool, session.workspace_id).await?
        else {
            return Ok(());
        };
        let lock = self.lock_for(row.issue_id);
        let _guard = lock.lock().await;
        let row = OrchestrationSubtask::find_by_id(pool, row.id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        // A manual follow-up in the subtask's workspace is not the coder.
        if row
            .execution_process_id
            .is_some_and(|id| id != execution_id)
        {
            return Ok(());
        }
        match row.status {
            SubtaskStatus::Running => {}
            SubtaskStatus::Stopped => {
                self.remove_container(&row).await;
                return self.tick_locked(row.issue_id).await;
            }
            _ => return Ok(()),
        }
        if row.execution_process_id.is_none() {
            OrchestrationSubtask::set_run(pool, row.id, execution_id).await?;
        }

        let result = match process.status {
            ExecutionProcessStatus::Completed => match sync_error(pool, execution_id).await? {
                Some(e) => Err(format!(
                    "Bringing the branch back from the sandbox failed: {e}"
                )),
                None => Ok(()),
            },
            ExecutionProcessStatus::Killed => {
                Err("The coder run was stopped (budget or time limit)".to_string())
            }
            _ => Err("The coder run failed; see its log in the subtask's workspace".to_string()),
        };
        match result {
            Ok(()) => self.merge_locked(&row).await?,
            Err(e) => self.fail_locked(&row, &e).await?,
        }
        self.remove_container(&row).await;
        self.tick_locked(row.issue_id).await
    }

    /// The coder's container goes when its run ends (D37); the host worktree
    /// stays as the review copy.
    async fn remove_container(&self, row: &OrchestrationSubtask) {
        if let Some(id) = row.workspace_id {
            self.container
                .sandbox()
                .remove_workspace_container(id)
                .await;
        }
    }

    async fn fail_locked(
        &self,
        row: &OrchestrationSubtask,
        error: &str,
    ) -> Result<(), OrchestratorError> {
        OrchestrationSubtask::set_status(&self.db.pool, row.id, SubtaskStatus::Failed, Some(error))
            .await?;
        self.move_issue(row.sub_issue_id, ProjectStatusKind::Blocked)
            .await;
        self.event(
            row.issue_id,
            IssueEventType::SubtaskFailed,
            "system",
            json!({"subtask": row.plan_subtask_id, "error": error}),
        )
        .await;
        Ok(())
    }

    /// Merges the subtask's branch into the ticket branch in every repo. On
    /// a conflict the subtask starts over on the updated ticket branch, with
    /// its earlier changes as context (D22).
    async fn merge_locked(&self, row: &OrchestrationSubtask) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let plan = Plan::find_by_id(pool, o.plan_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let title = plan
            .plan
            .0
            .subtasks
            .iter()
            .find(|s| s.id == row.plan_subtask_id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        let ticket_ws = match self.ticket_workspace(&o).await {
            Ok(ws) => ws,
            Err(e) => return self.fail_locked(row, &e.to_string()).await,
        };
        let sub_ws = match row.workspace_id {
            Some(id) => Workspace::find_by_id(pool, id).await?,
            None => None,
        }
        .ok_or(OrchestratorError::NotFound)?;
        let sub_root = PathBuf::from(sub_ws.container_ref.clone().unwrap_or_default());
        let id = self.display_id(&issue).await;
        let message = format!("{id}: Merge subtask {}: {title}", row.plan_subtask_id);

        let repos = WorkspaceRepo::find_repos_for_workspace(pool, ticket_ws.id).await?;
        let (commits, conflicts) = match self.merge_into_ticket(&ticket_ws, &sub_ws, &message).await
        {
            MergeOutcome::Merged(commits) => (commits, vec![]),
            MergeOutcome::Conflicts(conflicts) => (BTreeMap::new(), conflicts),
            MergeOutcome::Failed(e) => return self.fail_locked(row, &e).await,
        };

        if conflicts.is_empty() {
            OrchestrationSubtask::set_merged(pool, row.id, &commits).await?;
            // Its work lives on the ticket branch now; a merged subtask has
            // no business sitting under "Needs attention". Still in the
            // archive, and its branch stays.
            if let Err(e) = Workspace::update(pool, sub_ws.id, Some(true), None, None).await {
                tracing::warn!("Archiving the workspace of subtask {} failed: {e}", row.id);
            }
            self.move_issue(row.sub_issue_id, ProjectStatusKind::Done)
                .await;
            self.event(
                row.issue_id,
                IssueEventType::SubtaskMerged,
                "system",
                json!({"subtask": row.plan_subtask_id, "title": title,
                       "branch": sub_ws.branch, "into": ticket_ws.branch,
                       "commits": commits}),
            )
            .await;
            return Ok(());
        }

        let files: Vec<String> = conflicts
            .iter()
            .flat_map(|(repo, files)| files.iter().map(move |f| format!("{repo}/{f}")))
            .collect();
        let round = issue.fix_round + 1;
        if round > MAX_FIX_ROUNDS {
            return self
                .fail_locked(
                    row,
                    &format!(
                        "Merge conflicts in {} after {MAX_FIX_ROUNDS} fix rounds",
                        files.join(", ")
                    ),
                )
                .await;
        }
        let previous_diff = self.previous_diff(&ticket_ws, &sub_ws, &conflicts);
        // Start over from the ticket branch in every repo; repos that merged
        // cleanly already have the work there.
        for repo in &repos {
            if let Err(e) = self
                .git
                .reset_branch_to(&sub_root.join(&repo.name), &ticket_ws.branch)
            {
                return self
                    .fail_locked(
                        row,
                        &format!("Resetting the subtask branch in {}: {e}", repo.name),
                    )
                    .await;
            }
        }
        self.remove_container(row).await;
        OrchestrationSubtask::set_conflict(
            pool,
            row.id,
            &SubtaskConflict {
                files: files.clone(),
                previous_diff,
            },
        )
        .await?;
        Issue::set_fix_round(pool, issue.id, round).await?;
        self.event(
            row.issue_id,
            IssueEventType::MergeConflict,
            "system",
            json!({"subtask": row.plan_subtask_id, "title": title, "files": files,
                   "fix_round": round}),
        )
        .await;
        Ok(())
    }

    /// Merges `run_ws`'s branch into the ticket branch in every repo of the
    /// ticket's workspace. Conflicted merges are aborted; a repo that merged
    /// before another conflicted keeps its merge (the re-run then has
    /// nothing new for it).
    pub(crate) async fn merge_into_ticket(
        &self,
        ticket_ws: &Workspace,
        run_ws: &Workspace,
        message: &str,
    ) -> MergeOutcome {
        let repos = match WorkspaceRepo::find_repos_for_workspace(&self.db.pool, ticket_ws.id).await
        {
            Ok(repos) => repos,
            Err(e) => return MergeOutcome::Failed(e.to_string()),
        };
        let ticket_root = PathBuf::from(ticket_ws.container_ref.clone().unwrap_or_default());
        let mut commits = BTreeMap::new();
        let mut conflicts: Vec<(String, Vec<String>)> = Vec::new();
        for repo in &repos {
            let worktree = ticket_root.join(&repo.name);
            match self
                .git
                .merge_branch_no_ff(&worktree, &run_ws.branch, message)
            {
                Ok(Some(head)) => {
                    commits.insert(repo.name.clone(), head);
                }
                Ok(None) => {}
                Err(GitServiceError::MergeConflicts {
                    conflicted_files, ..
                }) => conflicts.push((repo.name.clone(), conflicted_files)),
                Err(e) => return MergeOutcome::Failed(format!("Merging into {}: {e}", repo.name)),
            }
        }
        if conflicts.is_empty() {
            MergeOutcome::Merged(commits)
        } else {
            MergeOutcome::Conflicts(conflicts)
        }
    }

    /// The earlier attempt's own changes, from where it forked off the
    /// ticket branch, for the repos that conflicted (capped).
    pub(crate) fn previous_diff(
        &self,
        ticket_ws: &Workspace,
        run_ws: &Workspace,
        conflicts: &[(String, Vec<String>)],
    ) -> String {
        let run_root = PathBuf::from(run_ws.container_ref.clone().unwrap_or_default());
        let mut previous_diff = String::new();
        for (repo, _) in conflicts {
            let range = format!("{}...{}", ticket_ws.branch, run_ws.branch);
            match self.git.diff_text(
                &run_root.join(repo),
                &range,
                PREVIOUS_DIFF_MAX_BYTES.saturating_sub(previous_diff.len()),
            ) {
                Ok(diff) if conflicts.len() > 1 => {
                    previous_diff.push_str(&format!("# Repository: {repo}\n{diff}"))
                }
                Ok(diff) => previous_diff.push_str(&diff),
                Err(e) => tracing::warn!("Diff of the earlier attempt in {repo} failed: {e}"),
            }
        }
        previous_diff
    }

    // ---- Stop and Retry ----------------------------------------------------

    async fn stop_run(&self, execution_id: Uuid) {
        let Ok(Some(process)) = ExecutionProcess::find_by_id(&self.db.pool, execution_id).await
        else {
            return;
        };
        if process.status != ExecutionProcessStatus::Running {
            return;
        }
        if let Err(e) = self
            .container
            .stop_execution(&process, ExecutionProcessStatus::Killed)
            .await
        {
            tracing::warn!("Stopping coder run {execution_id} failed: {e}");
        }
    }

    pub async fn stop_subtask(
        &self,
        subtask_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationSubtask::find_by_id(pool, subtask_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let run = {
            let lock = self.lock_for(row.issue_id);
            let _guard = lock.lock().await;
            let row = OrchestrationSubtask::find_by_id(pool, subtask_id)
                .await?
                .ok_or(OrchestratorError::NotFound)?;
            if !matches!(row.status, SubtaskStatus::Pending | SubtaskStatus::Running) {
                return Err(OrchestratorError::Invalid(format!(
                    "Subtask {} is already {}",
                    row.plan_subtask_id, row.status
                )));
            }
            OrchestrationSubtask::set_status(pool, row.id, SubtaskStatus::Stopped, None).await?;
            self.event(
                row.issue_id,
                IssueEventType::SubtaskStopped,
                "user",
                json!({"subtask": row.plan_subtask_id}),
            )
            .await;
            self.tick_locked(row.issue_id).await?;
            row.execution_process_id
        };
        if let Some(run) = run {
            self.stop_run(run).await;
        }
        self.view(row.issue_id).await
    }

    pub async fn stop_ticket(
        &self,
        issue_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let runs = {
            let lock = self.lock_for(issue_id);
            let _guard = lock.lock().await;
            let o = TicketOrchestration::find(pool, issue_id)
                .await?
                .ok_or(OrchestratorError::NotFound)?;
            if o.status != OrchestrationStatus::Running {
                return Err(OrchestratorError::Invalid(format!(
                    "Coding is already {}",
                    o.status
                )));
            }
            TicketOrchestration::set_status(pool, issue_id, OrchestrationStatus::Stopped, None)
                .await?;
            let mut runs = Vec::new();
            // Pending subtasks stay pending, so Retry continues where this
            // stopped.
            for row in OrchestrationSubtask::find_by_issue(pool, issue_id).await? {
                if row.status == SubtaskStatus::Running {
                    OrchestrationSubtask::set_status(pool, row.id, SubtaskStatus::Stopped, None)
                        .await?;
                    runs.extend(row.execution_process_id);
                }
            }
            for row in OrchestrationCheckpoint::find_by_issue(pool, issue_id).await? {
                if matches!(
                    row.status,
                    CheckpointStatus::Testing | CheckpointStatus::Fixing
                ) {
                    OrchestrationCheckpoint::set_status(
                        pool,
                        row.id,
                        CheckpointStatus::Stopped,
                        None,
                    )
                    .await?;
                    runs.extend(row.execution_process_id);
                }
            }
            self.event(
                issue_id,
                IssueEventType::CodingFinished,
                "user",
                json!({"status": "stopped"}),
            )
            .await;
            runs
        };
        for run in runs {
            self.stop_run(run).await;
        }
        self.view(issue_id).await
    }

    /// Back to running, after a stop or a block. Fix rounds start over: you
    /// looked at it and decided it is worth another go.
    async fn reopen_locked(&self, issue_id: Uuid) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        TicketOrchestration::set_status(pool, issue_id, OrchestrationStatus::Running, None).await?;
        Issue::set_fix_round(pool, issue_id, 0).await?;
        self.event(
            issue_id,
            IssueEventType::CodingStarted,
            "user",
            json!({"reason": "retry"}),
        )
        .await;
        self.move_issue(Some(issue_id), ProjectStatusKind::InProgress)
            .await;
        Ok(())
    }

    pub async fn retry_subtask(
        &self,
        subtask_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationSubtask::find_by_id(pool, subtask_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let lock = self.lock_for(row.issue_id);
        let guard = lock.lock().await;
        if !OrchestrationSubtask::requeue(pool, subtask_id).await? {
            return Err(OrchestratorError::Invalid(
                "Only failed or stopped subtasks can be retried".into(),
            ));
        }
        self.move_issue(row.sub_issue_id, ProjectStatusKind::Backlog)
            .await;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        if o.status != OrchestrationStatus::Running {
            self.reopen_locked(row.issue_id).await?;
        }
        self.tick_locked(row.issue_id).await?;
        drop(guard);
        self.view(row.issue_id).await
    }

    pub async fn retry_ticket(
        &self,
        issue_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let lock = self.lock_for(issue_id);
        let guard = lock.lock().await;
        let o = TicketOrchestration::find(pool, issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        if o.status == OrchestrationStatus::Running {
            return Err(OrchestratorError::Invalid(
                "Coding is already running".into(),
            ));
        }
        if o.status == OrchestrationStatus::Done {
            return Err(OrchestratorError::Invalid(
                "Every subtask is merged already".into(),
            ));
        }
        for row in OrchestrationSubtask::find_by_issue(pool, issue_id).await? {
            if OrchestrationSubtask::requeue(pool, row.id).await? {
                self.move_issue(row.sub_issue_id, ProjectStatusKind::Backlog)
                    .await;
            }
        }
        for row in OrchestrationCheckpoint::find_by_issue(pool, issue_id).await? {
            OrchestrationCheckpoint::requeue(pool, row.id).await?;
        }
        self.reopen_locked(issue_id).await?;
        self.tick_locked(issue_id).await?;
        drop(guard);
        self.view(issue_id).await
    }

    pub async fn stop_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let run = {
            let lock = self.lock_for(row.issue_id);
            let _guard = lock.lock().await;
            let row = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id)
                .await?
                .ok_or(OrchestratorError::NotFound)?;
            if !matches!(
                row.status,
                CheckpointStatus::Pending | CheckpointStatus::Testing | CheckpointStatus::Fixing
            ) {
                return Err(OrchestratorError::Invalid(format!(
                    "Checkpoint {} is already {}",
                    row.position + 1,
                    row.status
                )));
            }
            OrchestrationCheckpoint::set_status(pool, row.id, CheckpointStatus::Stopped, None)
                .await?;
            self.event(
                row.issue_id,
                IssueEventType::CheckpointFailed,
                "user",
                json!({"checkpoint": row.position + 1, "what": row.what, "stopped": true}),
            )
            .await;
            self.tick_locked(row.issue_id).await?;
            row.execution_process_id
        };
        if let Some(run) = run {
            self.stop_run(run).await;
        }
        self.view(row.issue_id).await
    }

    /// Tests a failed or stopped checkpoint again, from a fresh tester run.
    pub async fn retry_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let lock = self.lock_for(row.issue_id);
        let guard = lock.lock().await;
        if !OrchestrationCheckpoint::requeue(pool, checkpoint_id).await? {
            return Err(OrchestratorError::Invalid(
                "Only failed or stopped checkpoints can be retried".into(),
            ));
        }
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        if o.status != OrchestrationStatus::Running {
            self.reopen_locked(row.issue_id).await?;
        }
        self.tick_locked(row.issue_id).await?;
        drop(guard);
        self.view(row.issue_id).await
    }

    /// Marks an issue found as won't fix (or opens it again). A won't-fix
    /// issue goes to no fix run and the tester is told to leave it.
    pub async fn set_issue_found_status(
        &self,
        id: Uuid,
        status: IssueFoundStatus,
    ) -> Result<OrchestrationView, OrchestratorError> {
        let pool = &self.db.pool;
        let found = IssueFound::find_by_id(pool, id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        if status == IssueFoundStatus::Fixed {
            return Err(OrchestratorError::Invalid(
                "Issues are marked fixed by a re-test".into(),
            ));
        }
        IssueFound::set_status(pool, id, status).await?;
        self.event(
            found.issue_id,
            IssueEventType::IssueFoundUpdated,
            "user",
            json!({"issue_found": id, "status": status, "description": found.description}),
        )
        .await;
        self.view(found.issue_id).await
    }
}

/// The error of the run's last sync out of the sandbox, if it had one.
async fn sync_error(
    pool: &sqlx::SqlitePool,
    execution_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    Ok(
        ExecutionProcessEvent::find_by_execution_process(pool, execution_id)
            .await?
            .into_iter()
            .filter(|e| e.event_type == ExecutionProcessEventType::Sandbox)
            .filter(|e| e.payload.0.get("action").and_then(|a| a.as_str()) == Some("sync_out"))
            .filter_map(|e| {
                e.payload
                    .0
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .next_back(),
    )
}

async fn workspace_cost(pool: &sqlx::SqlitePool, workspace_id: Uuid) -> Result<f64, sqlx::Error> {
    sqlx::query_scalar::<_, f64>(
        "SELECT COALESCE(SUM(ep.cost_usd), 0.0) FROM execution_processes ep \
         JOIN sessions s ON s.id = ep.session_id WHERE s.workspace_id = ?",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plan() -> PlanDocument {
        PlanDocument::from_value(json!({
            "subtasks": [
                {"id": "1", "title": "Model", "files": ["shop/src/model.rs"]},
                {"id": "2", "title": "Mail", "description": "Send it.",
                 "files": ["shop/src/mail.rs"]},
                {"id": "3", "title": "Route", "files": ["shop/src/route.rs"],
                 "depends_on": ["1", "2"]}
            ],
            "acceptance_criteria": ["Links expire after 1 h"]
        }))
        .unwrap()
    }

    #[test]
    fn coder_task_names_the_context_and_the_rules() {
        let doc = plan();
        let task = coder_task("PROJ-1", &doc.subtasks[1], 2, 3, false);
        assert!(task.contains("subtask 2 of ticket PROJ-1: \"Mail\" (2 of 3)"));
        assert!(task.contains("/kanbain/context/subtask.md"));
        assert!(task.contains("`PROJ-1: `"));
        assert!(task.contains("Do not create or switch branches"));
        assert!(!task.contains("conflict.md"));
        assert!(coder_task("PROJ-1", &doc.subtasks[1], 2, 3, true).contains("conflict.md"));
    }

    #[test]
    fn subtask_md_lists_merged_and_parallel_work() {
        let doc = plan();
        let schedule = Schedule::new(&doc);
        use SubtaskStatus::*;
        let md = render_subtask(&doc, 1, &schedule, &[Running, Running, Pending]);
        assert!(md.starts_with("# Subtask 2 of 3: Mail\n\nSend it."));
        assert!(md.contains("- shop/src/mail.rs"));
        assert!(md.contains("Running in parallel"));
        assert!(md.contains("- 1: Model (shop/src/model.rs)"));
        assert!(md.contains("- Links expire after 1 h"));
        assert!(!md.contains("Already merged"));
        let md = render_subtask(&doc, 2, &schedule, &[Merged, Merged, Running]);
        assert!(md.contains("Already merged into your branch\n\n- 1: Model\n- 2: Mail\n"));
        assert!(!md.contains("Running in parallel"));
    }

    #[test]
    fn conflict_md_points_at_the_earlier_diff() {
        let md = render_conflict(
            &SubtaskConflict {
                files: vec!["shop/src/mail.rs".into()],
                previous_diff: String::new(),
            },
            "kanbain/PROJ-1-x",
            1,
        );
        assert!(md.contains("fix round 1 of 3"));
        assert!(md.contains("- shop/src/mail.rs"));
        assert!(md.contains("`kanbain/PROJ-1-x`"));
        assert!(md.contains("git apply --3way /kanbain/context/previous_attempt.diff"));
    }
}
