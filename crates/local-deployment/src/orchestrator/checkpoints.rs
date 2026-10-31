//! The tester and the fix loop (Phase 6, D23, D24).
//!
//! A checkpoint whose subtasks are merged gets a **tester** run on its own
//! branch (`<ticket branch>--test-<n>`), branched from the ticket branch.
//! The tester writes black-box tests and `issues.json`. When it ends:
//! - Everything it changed outside the project's test paths is dropped, and
//!   the rest is squashed into one test commit (the fence; the prompt only
//!   asks, this enforces).
//! - That commit is merged into the ticket branch, and the issues land in
//!   `issues_found`.
//! - No open issues: the checkpoint passed. Otherwise a **fix** run (the
//!   coder agent) works on `<ticket branch>--fix-<n>` with the open issues,
//!   is merged, and the tester runs again. Every fix run is a fix round;
//!   past [`MAX_FIX_ROUNDS`] (shared with merge-conflict re-runs) the
//!   checkpoint fails and the ticket goes to Blocked.

use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
    issue::Issue,
    issue_event::IssueEventType,
    issue_found::{IssueFound, IssueFoundStatus, parse_issues_json},
    orchestration::{
        CheckpointStatus, OrchestrationCheckpoint, SubtaskConflict, TicketOrchestration,
    },
    plan::{Plan, PlanDocument, PlanSubtask},
    project::Project,
    requests::WorkspaceRepoInput,
    session::{CreateSession, Session},
    workspace::{CreateWorkspace, Workspace},
    workspace_repo::WorkspaceRepo,
};
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType, coding_agent_initial::CodingAgentInitialRequest,
    },
    adapters::{AgentRole, executor_for},
    model_selector::PermissionPolicy,
    profile::ExecutorConfig,
};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;
use serde_json::json;
use services::services::{container::ContainerService, orchestrator::FINAL_CHECKPOINT};
use ts_rs::TS;
use uuid::Uuid;

use super::{
    MAX_FIX_ROUNDS, MergeOutcome, OrchestratorError, OrchestratorService, render_conflict,
    sync_error, workspace_cost,
};

/// One checkpoint as the ticket panel shows it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CheckpointView {
    #[serde(flatten)]
    #[ts(flatten)]
    pub checkpoint: OrchestrationCheckpoint,
    /// Titles of the subtasks it tests.
    pub after_titles: Vec<String>,
    pub is_final: bool,
    pub tester_branch: Option<String>,
    pub fixer_branch: Option<String>,
    /// Model cost of its tester and fix runs.
    pub cost_usd: f64,
}

/// Whether a repo-relative path is under the test paths. Invalid globs are
/// skipped (and logged); with none left nothing is a test path, which is
/// the safe side of a fence.
pub fn test_path_matcher(patterns: &[String]) -> impl Fn(&str) -> bool + use<> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        match GlobBuilder::new(pattern.trim().trim_start_matches("./"))
            .literal_separator(true)
            .build()
        {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(e) => tracing::warn!("Skipping the test path `{pattern}`: {e}"),
        }
    }
    let set = builder.build().unwrap_or_else(|_| GlobSet::empty());
    move |path: &str| set.is_match(path)
}

/// The first message of a tester run.
pub fn tester_task(id: &str, n: usize, what: &str, round: i64, rerun: bool) -> String {
    let mut task = format!(
        "Test checkpoint {n} of ticket {id}: \"{what}\".{}\n\n\
         Read these first (read-only):\n\
         - /kanbain/context/checkpoint.md: what to test, the test paths you may write to, \
         earlier issues and the issues.json format.\n\
         - /kanbain/context/ticket.md: the ticket and its requirements.\n\
         - /kanbain/context/plan.json: the approved plan, for context only.\n",
        if round > 0 {
            format!(" This is a re-test after fix round {round}.")
        } else {
            String::new()
        }
    );
    if rerun {
        task.push_str(
            "- /kanbain/context/conflict.md: why you are running again. Read it before \
             anything else.\n",
        );
    }
    task.push_str(&format!(
        "\nRules:\n\
         - Write black-box tests from the ticket's requirements and acceptance criteria, \
         not from the implementation. Read the code only to learn how to call it.\n\
         - Add or change only files under the test paths in checkpoint.md. KanbAIn drops \
         every other change you make.\n\
         - Run the tests. Keep a failing test that shows a real problem: it is how the \
         coder sees it.\n\
         - Leave your tests in the repository under /workspace; you need not commit. \
         KanbAIn commits them as one `{id}: Tests for checkpoint {n}` commit when you \
         finish.\n\
         - Write /kanbain/out/issues.json, also when you found nothing (`[]`).\n\
         - Do not create or switch branches, merge, rebase or push.\n"
    ));
    task
}

/// The first message of a fix run.
pub fn fixer_task(id: &str, n: usize, what: &str, round: i64, rerun: bool) -> String {
    let mut task = format!(
        "Fix the issues the tester found at checkpoint {n} of ticket {id}: \"{what}\" \
         (fix round {round} of {MAX_FIX_ROUNDS}).\n\n\
         Read these first (read-only):\n\
         - /kanbain/context/issues.md: the open issues, with file, line and severity.\n\
         - /kanbain/context/ticket.md: the ticket.\n\
         - /kanbain/context/plan.json: the approved plan, for context only.\n\
         - /kanbain/context/related.md: code locations search found for these issues.\n"
    );
    if rerun {
        task.push_str(
            "- /kanbain/context/conflict.md: why you are running again. Read it before \
             anything else.\n",
        );
    }
    task.push_str(&format!(
        "\nRules:\n\
         - Fix the code so the problems are gone and the tester's tests pass. Do not \
         delete or weaken the tests; if one is wrong, say why in your final message.\n\
         - Commit your work in the repository under /workspace with messages starting \
         `{id}: `. Leave nothing uncommitted.\n\
         - Do not create or switch branches, merge, rebase or push. KanbAIn merges your \
         branch into the ticket branch when you finish.\n"
    ));
    task
}

fn issue_line(i: &IssueFound) -> String {
    let at = match (&i.file, i.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.clone(),
        _ => "(no file)".to_string(),
    };
    format!(
        "- [{}] {} · {} · {}: {}\n",
        i.id,
        i.severity,
        i.category.to_string().replace('_', " "),
        at,
        i.description.trim()
    )
}

/// `checkpoint.md` for a tester run.
pub fn render_checkpoint(
    plan: &PlanDocument,
    n: usize,
    checkpoint: &OrchestrationCheckpoint,
    test_paths: &[String],
    issues: &[IssueFound],
) -> String {
    let mut out = format!("# Checkpoint {n}: {}\n", checkpoint.what);
    let covered: Vec<&PlanSubtask> = plan
        .subtasks
        .iter()
        .filter(|s| checkpoint.after_subtasks.0.contains(&s.id))
        .collect();
    if !covered.is_empty() {
        out.push_str("\n## Merged work it covers\n\n");
        for s in covered {
            out.push_str(&format!("- {}: {}\n", s.id, s.title));
        }
    }
    if !plan.acceptance_criteria.is_empty() {
        out.push_str("\n## The ticket's acceptance criteria\n\n");
        for c in &plan.acceptance_criteria {
            out.push_str(&format!("- {c}\n"));
        }
    }
    if !plan.test_strategy.trim().is_empty() {
        out.push_str(&format!(
            "\n## The plan's test strategy\n\n{}\n",
            plan.test_strategy.trim()
        ));
    }
    out.push_str(
        "\n## Test paths\n\nYou may add or change only files matching these globs, relative \
         to each repository. Everything else you change is dropped.\n\n",
    );
    for p in test_paths {
        out.push_str(&format!("- `{p}`\n"));
    }
    let open: Vec<&IssueFound> = issues
        .iter()
        .filter(|i| i.status == IssueFoundStatus::Open)
        .collect();
    if !open.is_empty() {
        out.push_str(
            "\n## Issues from the last test run\n\nA coder tried to fix these. Check each \
             one. If it is still there, report it again with its `id`; if it is fixed, \
             leave it out.\n\n",
        );
        for i in open {
            out.push_str(&issue_line(i));
        }
    }
    let accepted: Vec<&IssueFound> = issues
        .iter()
        .filter(|i| i.status == IssueFoundStatus::Wontfix)
        .collect();
    if !accepted.is_empty() {
        out.push_str("\n## Accepted as they are (do not report these)\n\n");
        for i in accepted {
            out.push_str(&issue_line(i));
        }
    }
    out.push_str(
        "\n## issues.json\n\nWrite `/kanbain/out/issues.json`: a JSON list, `[]` when \
         everything works. One entry per problem:\n\n\
         ```json\n\
         [{\"id\": \"<only when repeating an earlier issue>\", \"file\": \"repo/src/x.rs\", \
         \"line\": 42, \"severity\": \"low | medium | high\", \
         \"category\": \"edge case | integration | logic | error handling | state/timing | \
         requirements\", \"description\": \"what goes wrong, and which test shows it\"}]\n\
         ```\n",
    );
    out
}

/// `issues.md` for a fix run.
pub fn render_issues(n: usize, what: &str, issues: &[IssueFound]) -> String {
    let mut out = format!(
        "# Open issues at checkpoint {n}: {what}\n\n\
         The tester's tests for them are already on your branch.\n\n"
    );
    for i in issues.iter().filter(|i| i.status == IssueFoundStatus::Open) {
        out.push_str(&issue_line(i));
    }
    out
}

/// What a finished tester run handed in.
enum TesterReport {
    Issues(Vec<db::models::issue_found::ReportedIssue>),
    Missing,
    Invalid(String),
}

fn tester_report(files: &serde_json::Map<String, serde_json::Value>) -> TesterReport {
    match files.get("issues.json").and_then(|v| v.as_str()) {
        None => TesterReport::Missing,
        Some(text) => match parse_issues_json(text) {
            Ok(issues) => TesterReport::Issues(issues),
            Err(e) => TesterReport::Invalid(e.to_string()),
        },
    }
}

/// The run's harvested `/kanbain/out` files.
async fn harvested(
    pool: &sqlx::SqlitePool,
    execution_id: Uuid,
) -> Result<serde_json::Map<String, serde_json::Value>, sqlx::Error> {
    Ok(
        ExecutionProcessEvent::find_by_execution_process(pool, execution_id)
            .await?
            .into_iter()
            .filter(|e| e.event_type == ExecutionProcessEventType::Sandbox)
            .filter_map(|e| {
                (e.payload.0.get("action")?.as_str()? == "harvest")
                    .then(|| e.payload.0.get("files")?.as_object().cloned())
                    .flatten()
            })
            .next_back()
            .unwrap_or_default(),
    )
}

impl OrchestratorService {
    // ---- Rows and views ----------------------------------------------------

    /// The ticket's checkpoint rows, created from the schedule when missing
    /// (a plan approved before Phase 6 has none).
    pub(super) async fn ensure_checkpoints(
        &self,
        issue_id: Uuid,
        doc: &PlanDocument,
        schedule: &services::services::orchestrator::Schedule,
    ) -> Result<Vec<OrchestrationCheckpoint>, OrchestratorError> {
        let pool = &self.db.pool;
        let mut rows = OrchestrationCheckpoint::find_by_issue(pool, issue_id).await?;
        if rows.len() == schedule.checkpoints().len() {
            return Ok(rows);
        }
        for (position, c) in schedule.checkpoints().iter().enumerate() {
            if rows.iter().any(|r| r.position == position as i64) {
                continue;
            }
            let after: Vec<String> = c
                .after
                .iter()
                .map(|&i| doc.subtasks[i].id.clone())
                .collect();
            OrchestrationCheckpoint::create(pool, issue_id, position as i64, &c.what, &after)
                .await?;
        }
        rows = OrchestrationCheckpoint::find_by_issue(pool, issue_id).await?;
        Ok(rows)
    }

    pub(super) async fn checkpoint_views(
        &self,
        issue_id: Uuid,
        doc: &PlanDocument,
    ) -> Result<Vec<CheckpointView>, OrchestratorError> {
        let pool = &self.db.pool;
        let mut views = Vec::new();
        for row in OrchestrationCheckpoint::find_by_issue(pool, issue_id).await? {
            let after_titles = doc
                .subtasks
                .iter()
                .filter(|s| row.after_subtasks.0.contains(&s.id))
                .map(|s| s.title.clone())
                .collect();
            let mut cost_usd = 0.0;
            let branch = |id: Option<Uuid>| async move {
                match id {
                    Some(id) => Workspace::find_by_id(pool, id)
                        .await
                        .ok()
                        .flatten()
                        .map(|w| w.branch),
                    None => None,
                }
            };
            let tester_branch = branch(row.tester_workspace_id).await;
            let fixer_branch = branch(row.fixer_workspace_id).await;
            for id in [row.tester_workspace_id, row.fixer_workspace_id]
                .into_iter()
                .flatten()
            {
                cost_usd += workspace_cost(pool, id).await?;
            }
            views.push(CheckpointView {
                is_final: row.what == FINAL_CHECKPOINT,
                after_titles,
                tester_branch,
                fixer_branch,
                cost_usd,
                checkpoint: row,
            });
        }
        Ok(views)
    }

    // ---- Launching a tester or fix run --------------------------------------

    /// Starts the checkpoint's current step: a tester run while `testing`,
    /// a fix run while `fixing`. Boxed like [`Self::launch`].
    pub(super) fn launch_check(
        &self,
        checkpoint_id: Uuid,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let Err(e) = self.launch_check_inner(checkpoint_id).await else {
                return;
            };
            if self.is_shutting_down() {
                return;
            }
            tracing::warn!("Starting the run for checkpoint {checkpoint_id} failed: {e}");
            let pool = &self.db.pool;
            let Ok(Some(row)) = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id).await
            else {
                return;
            };
            let lock = self.lock_for(row.issue_id);
            let _guard = lock.lock().await;
            let Ok(Some(row)) = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id).await
            else {
                return;
            };
            if matches!(
                row.status,
                CheckpointStatus::Testing | CheckpointStatus::Fixing
            ) {
                self.fail_check_locked(&row, &e.to_string()).await;
            }
            let _ = self.tick_locked(row.issue_id).await;
        })
    }

    async fn launch_check_inner(&self, checkpoint_id: Uuid) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let row = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let fixer = match row.status {
            CheckpointStatus::Testing => false,
            CheckpointStatus::Fixing => true,
            // Stopped (or settled) before the launch got here.
            _ => return Ok(()),
        };
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let ticket_ws = self.ticket_workspace(&o).await?;
        let workspace = self
            .check_workspace(&issue, &ticket_ws, &row, fixer)
            .await?;

        let role = if fixer {
            AgentRole::Coder
        } else {
            AgentRole::Tester
        };
        let agent_name = self
            .jobs
            .agent_name_for(role)
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
        let n = row.position as usize + 1;
        let session = Session::create(
            pool,
            &CreateSession {
                executor: Some(executor_config.executor.to_string()),
                name: Some(if fixer {
                    format!("Fixer · checkpoint {n}, round {}", issue.fix_round)
                } else {
                    format!("Tester · checkpoint {n}")
                }),
            },
            Uuid::new_v4(),
            workspace.id,
        )
        .await
        .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        let id = self.display_id(&issue).await;
        let rerun = row.conflict.is_some();
        let prompt = if fixer {
            fixer_task(&id, n, &row.what, issue.fix_round, rerun)
        } else {
            tester_task(&id, n, &row.what, row.round, rerun)
        };
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
                &db::models::execution_process::ExecutionProcessRunReason::CodingAgent,
            )
            .await
            .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;

        let stop = {
            let lock = self.lock_for(row.issue_id);
            let _guard = lock.lock().await;
            let current = OrchestrationCheckpoint::find_by_id(pool, checkpoint_id).await?;
            if current
                .as_ref()
                .is_some_and(|c| c.execution_process_id.is_none())
            {
                OrchestrationCheckpoint::set_run(pool, checkpoint_id, execution.id).await?;
            }
            current.is_some_and(|c| c.status == CheckpointStatus::Stopped)
        };
        if stop {
            self.stop_run(execution.id).await;
        }
        Ok(())
    }

    /// The checkpoint's tester or fix workspace, on `<ticket branch>--test-<n>`
    /// or `--fix-<n>`. Each run starts from the current ticket branch: the
    /// last run's work is merged there already (or was given up on).
    async fn check_workspace(
        &self,
        issue: &Issue,
        ticket_ws: &Workspace,
        row: &OrchestrationCheckpoint,
        fixer: bool,
    ) -> Result<Workspace, OrchestratorError> {
        let pool = &self.db.pool;
        let existing = if fixer {
            row.fixer_workspace_id
        } else {
            row.tester_workspace_id
        };
        if let Some(id) = existing
            && let Some(workspace) = Workspace::find_by_id(pool, id).await?
        {
            // A container left from an earlier run holds a stale clone.
            self.container
                .sandbox()
                .remove_workspace_container(id)
                .await;
            self.container
                .ensure_container_exists(&workspace)
                .await
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
            let workspace = Workspace::find_by_id(pool, id).await?.unwrap_or(workspace);
            let root =
                std::path::PathBuf::from(workspace.container_ref.clone().unwrap_or_default());
            for repo in WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await? {
                self.git
                    .reset_branch_to(&root.join(&repo.name), &ticket_ws.branch)
                    .map_err(|e| {
                        OrchestratorError::Invalid(format!(
                            "Resetting {} onto the ticket branch: {e}",
                            repo.name
                        ))
                    })?;
            }
            return Ok(workspace);
        }
        let n = row.position + 1;
        let base = format!(
            "{}--{}-{n}",
            ticket_ws.branch,
            if fixer { "fix" } else { "test" }
        );
        let mut branch = base.clone();
        let mut k = 2;
        while Workspace::branch_in_use(pool, &branch).await? {
            branch = format!("{base}-{k}");
            k += 1;
        }
        let id = self.display_id(issue).await;
        let workspace = Workspace::create(
            pool,
            &CreateWorkspace {
                branch,
                name: Some(format!(
                    "{id} · {} checkpoint {n}",
                    if fixer { "Fix" } else { "Test" }
                )),
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
        OrchestrationCheckpoint::set_workspace(pool, row.id, fixer, workspace.id).await?;
        Ok(Workspace::find_by_id(pool, workspace.id)
            .await?
            .unwrap_or(workspace))
    }

    // ---- Context files -----------------------------------------------------

    /// `/kanbain/context` for a tester (`checkpoint.md`) or fix run
    /// (`issues.md`, `related.md`), with `plan.json` and, after a merge
    /// conflict, `conflict.md` and `previous_attempt.diff`.
    pub(super) async fn check_context_files(
        &self,
        row: &OrchestrationCheckpoint,
        workspace_id: Uuid,
    ) -> Result<Vec<(String, String)>, OrchestratorError> {
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
        let doc = &plan.plan.0;
        let n = row.position as usize + 1;
        let issues: Vec<IssueFound> = IssueFound::find_by_issue(pool, row.issue_id)
            .await?
            .into_iter()
            .filter(|i| i.checkpoint == Some(row.position))
            .collect();
        let mut files = vec![(
            "plan.json".to_string(),
            serde_json::to_string_pretty(doc).unwrap_or_default(),
        )];
        if row.fixer_workspace_id == Some(workspace_id) {
            files.push((
                "issues.md".to_string(),
                render_issues(n, &row.what, &issues),
            ));
            let open: Vec<&IssueFound> = issues
                .iter()
                .filter(|i| i.status == IssueFoundStatus::Open)
                .collect();
            let pseudo = PlanSubtask {
                id: format!("fix-{n}"),
                title: row.what.clone(),
                description: open
                    .iter()
                    .map(|i| i.description.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                files: open.iter().filter_map(|i| i.file.clone()).collect(),
                depends_on: vec![],
            };
            let id = self.display_id(&issue).await;
            files.push((
                "related.md".to_string(),
                self.related_code(&issue, &id, &pseudo).await,
            ));
        } else {
            let test_paths = Project::find_by_id(pool, issue.project_id)
                .await?
                .map(|p| p.settings.0.effective_test_paths())
                .unwrap_or_default();
            files.push((
                "checkpoint.md".to_string(),
                render_checkpoint(doc, n, row, &test_paths, &issues),
            ));
        }
        if let Some(conflict) = &row.conflict {
            let ticket_branch = match o.workspace_id {
                Some(id) => Workspace::find_by_id(pool, id).await?.map(|w| w.branch),
                None => None,
            }
            .unwrap_or_default();
            files.push((
                "conflict.md".to_string(),
                render_conflict(&conflict.0, &ticket_branch, issue.fix_round),
            ));
            files.push((
                "previous_attempt.diff".to_string(),
                conflict.0.previous_diff.clone(),
            ));
        }
        Ok(files)
    }

    // ---- When a tester or fix run ends -------------------------------------

    pub(super) async fn fail_check_locked(&self, row: &OrchestrationCheckpoint, error: &str) {
        let pool = &self.db.pool;
        if let Err(e) =
            OrchestrationCheckpoint::set_status(pool, row.id, CheckpointStatus::Failed, Some(error))
                .await
        {
            tracing::warn!("Failing checkpoint {} failed: {e}", row.id);
        }
        self.event(
            row.issue_id,
            IssueEventType::CheckpointFailed,
            "system",
            json!({"checkpoint": row.position + 1, "what": row.what, "error": error}),
        )
        .await;
    }

    /// Handles the end of a tester or fix run. Caller holds the ticket lock
    /// and has checked the run is the checkpoint's current one.
    pub(super) async fn finish_check_locked(
        &self,
        row: &OrchestrationCheckpoint,
        process: &ExecutionProcess,
        workspace_id: Uuid,
    ) -> Result<(), OrchestratorError> {
        let pool = &self.db.pool;
        let fixer = match row.status {
            CheckpointStatus::Testing if row.tester_workspace_id == Some(workspace_id) => false,
            CheckpointStatus::Fixing if row.fixer_workspace_id == Some(workspace_id) => true,
            CheckpointStatus::Stopped => {
                self.container
                    .sandbox()
                    .remove_workspace_container(workspace_id)
                    .await;
                return self.tick_locked(row.issue_id).await;
            }
            _ => return Ok(()),
        };
        if row.execution_process_id.is_none() {
            OrchestrationCheckpoint::set_run(pool, row.id, process.id).await?;
        }
        let what = if fixer { "fix" } else { "tester" };
        let result = match process.status {
            ExecutionProcessStatus::Completed => match sync_error(pool, process.id).await? {
                Some(e) => Err(format!(
                    "Bringing the branch back from the sandbox failed: {e}"
                )),
                None => Ok(()),
            },
            ExecutionProcessStatus::Killed => {
                Err(format!("The {what} run was stopped (budget or time limit)"))
            }
            _ => Err(format!(
                "The {what} run failed; see its log in the checkpoint's workspace"
            )),
        };
        let relaunch = match result {
            Err(e) => {
                self.fail_check_locked(row, &e).await;
                false
            }
            Ok(()) if fixer => self.fix_finished_locked(row, workspace_id).await?,
            Ok(()) => {
                self.tester_finished_locked(row, process, workspace_id)
                    .await?
            }
        };
        self.container
            .sandbox()
            .remove_workspace_container(workspace_id)
            .await;
        if relaunch {
            let service = self.clone();
            let id = row.id;
            tokio::spawn(async move { service.launch_check(id).await });
        }
        self.tick_locked(row.issue_id).await
    }

    /// Fences, merges and reads a tester run. True when the checkpoint's
    /// next run (a fix, or the tester again after a conflict) should start.
    async fn tester_finished_locked(
        &self,
        row: &OrchestrationCheckpoint,
        process: &ExecutionProcess,
        workspace_id: Uuid,
    ) -> Result<bool, OrchestratorError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let ticket_ws = match self.ticket_workspace(&o).await {
            Ok(ws) => ws,
            Err(e) => {
                self.fail_check_locked(row, &e.to_string()).await;
                return Ok(false);
            }
        };
        let test_ws = Workspace::find_by_id(pool, workspace_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let test_root = std::path::PathBuf::from(test_ws.container_ref.clone().unwrap_or_default());
        let id = self.display_id(&issue).await;
        let n = row.position + 1;
        let test_paths = Project::find_by_id(pool, issue.project_id)
            .await?
            .map(|p| p.settings.0.effective_test_paths())
            .unwrap_or_default();
        let is_test = test_path_matcher(&test_paths);

        // The fence (D23): one test commit per repo, test paths only.
        let mut rejected = Vec::new();
        let repos = WorkspaceRepo::find_repos_for_workspace(pool, test_ws.id).await?;
        for repo in &repos {
            let message = if row.round > 0 {
                format!("{id}: Tests for checkpoint {n} (re-test {})", row.round)
            } else {
                format!("{id}: Tests for checkpoint {n}")
            };
            match self.git.squash_keeping(
                &test_root.join(&repo.name),
                &ticket_ws.branch,
                &is_test,
                &message,
            ) {
                Ok(squashed) => rejected.extend(
                    squashed
                        .dropped
                        .into_iter()
                        .map(|p| format!("{}/{p}", repo.name)),
                ),
                Err(e) => {
                    self.fail_check_locked(
                        row,
                        &format!("Keeping only the tests in {}: {e}", repo.name),
                    )
                    .await;
                    return Ok(false);
                }
            }
        }
        OrchestrationCheckpoint::set_rejected_files(pool, row.id, &rejected).await?;

        let message = format!("{id}: Merge tests for checkpoint {n}");
        let commits = match self.merge_into_ticket(&ticket_ws, &test_ws, &message).await {
            MergeOutcome::Merged(commits) => commits,
            MergeOutcome::Conflicts(conflicts) => {
                return self
                    .check_conflict_locked(row, &issue, &ticket_ws, &test_ws, conflicts)
                    .await;
            }
            MergeOutcome::Failed(e) => {
                self.fail_check_locked(row, &e).await;
                return Ok(false);
            }
        };

        let files = harvested(pool, process.id).await?;
        let reported = match tester_report(&files) {
            TesterReport::Issues(issues) => issues,
            TesterReport::Missing => {
                self.fail_check_locked(row, "The tester wrote no /kanbain/out/issues.json")
                    .await;
                return Ok(false);
            }
            TesterReport::Invalid(e) => {
                self.fail_check_locked(row, &e).await;
                return Ok(false);
            }
        };
        let results = IssueFound::record_test_results(
            pool,
            issue.id,
            Some(process.id),
            row.position,
            issue.fix_round,
            &reported,
        )
        .await?;
        self.event(
            issue.id,
            IssueEventType::TestsFinished,
            &format!("agent:{}", process.id),
            json!({"checkpoint": n, "what": row.what, "round": row.round,
                   "new": results.new, "still_open": results.still_open,
                   "fixed": results.fixed, "rejected": rejected, "commits": commits}),
        )
        .await;

        let open = IssueFound::open_at(pool, issue.id, row.position).await?;
        if open.is_empty() {
            OrchestrationCheckpoint::set_status(pool, row.id, CheckpointStatus::Passed, None)
                .await?;
            self.event(
                issue.id,
                IssueEventType::CheckpointPassed,
                "system",
                json!({"checkpoint": n, "what": row.what, "round": row.round}),
            )
            .await;
            return Ok(false);
        }
        let round = issue.fix_round + 1;
        if round > MAX_FIX_ROUNDS {
            self.fail_check_locked(
                row,
                &format!(
                    "{} open issue{} after {MAX_FIX_ROUNDS} fix rounds",
                    open.len(),
                    if open.len() == 1 { "" } else { "s" }
                ),
            )
            .await;
            return Ok(false);
        }
        Issue::set_fix_round(pool, issue.id, round).await?;
        OrchestrationCheckpoint::next_run(pool, row.id, CheckpointStatus::Fixing, row.round + 1)
            .await?;
        self.event(
            issue.id,
            IssueEventType::FixStarted,
            "system",
            json!({"checkpoint": n, "what": row.what, "fix_round": round,
                   "issues": open.len()}),
        )
        .await;
        Ok(true)
    }

    /// Merges a fix run; the tester then checks the checkpoint again.
    async fn fix_finished_locked(
        &self,
        row: &OrchestrationCheckpoint,
        workspace_id: Uuid,
    ) -> Result<bool, OrchestratorError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let o = TicketOrchestration::find(pool, row.issue_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let ticket_ws = match self.ticket_workspace(&o).await {
            Ok(ws) => ws,
            Err(e) => {
                self.fail_check_locked(row, &e.to_string()).await;
                return Ok(false);
            }
        };
        let fix_ws = Workspace::find_by_id(pool, workspace_id)
            .await?
            .ok_or(OrchestratorError::NotFound)?;
        let id = self.display_id(&issue).await;
        let n = row.position + 1;
        let message = format!(
            "{id}: Merge fixes for checkpoint {n} (fix round {})",
            issue.fix_round
        );
        let commits = match self.merge_into_ticket(&ticket_ws, &fix_ws, &message).await {
            MergeOutcome::Merged(commits) => commits,
            MergeOutcome::Conflicts(conflicts) => {
                return self
                    .check_conflict_locked(row, &issue, &ticket_ws, &fix_ws, conflicts)
                    .await;
            }
            MergeOutcome::Failed(e) => {
                self.fail_check_locked(row, &e).await;
                return Ok(false);
            }
        };
        self.event(
            issue.id,
            IssueEventType::FixMerged,
            "system",
            json!({"checkpoint": n, "what": row.what, "fix_round": issue.fix_round,
                   "commits": commits, "empty": commits.is_empty()}),
        )
        .await;
        OrchestrationCheckpoint::next_run(pool, row.id, CheckpointStatus::Testing, row.round)
            .await?;
        self.event(
            issue.id,
            IssueEventType::TesterStarted,
            "system",
            json!({"checkpoint": n, "what": row.what, "round": row.round}),
        )
        .await;
        Ok(true)
    }

    /// A tester or fix branch that conflicts with the ticket branch starts
    /// over on it, like a subtask (D22), and costs a fix round.
    async fn check_conflict_locked(
        &self,
        row: &OrchestrationCheckpoint,
        issue: &Issue,
        ticket_ws: &Workspace,
        run_ws: &Workspace,
        conflicts: Vec<(String, Vec<String>)>,
    ) -> Result<bool, OrchestratorError> {
        let pool = &self.db.pool;
        let files: Vec<String> = conflicts
            .iter()
            .flat_map(|(repo, files)| files.iter().map(move |f| format!("{repo}/{f}")))
            .collect();
        let round = issue.fix_round + 1;
        if round > MAX_FIX_ROUNDS {
            self.fail_check_locked(
                row,
                &format!(
                    "Merge conflicts in {} after {MAX_FIX_ROUNDS} fix rounds",
                    files.join(", ")
                ),
            )
            .await;
            return Ok(false);
        }
        let previous_diff = self.previous_diff(ticket_ws, run_ws, &conflicts);
        let root = std::path::PathBuf::from(run_ws.container_ref.clone().unwrap_or_default());
        for repo in WorkspaceRepo::find_repos_for_workspace(pool, run_ws.id).await? {
            if let Err(e) = self
                .git
                .reset_branch_to(&root.join(&repo.name), &ticket_ws.branch)
            {
                self.fail_check_locked(row, &format!("Resetting the branch in {}: {e}", repo.name))
                    .await;
                return Ok(false);
            }
        }
        OrchestrationCheckpoint::set_conflict(
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
            issue.id,
            IssueEventType::MergeConflict,
            "system",
            json!({"checkpoint": row.position + 1, "title": row.what, "files": files,
                   "fix_round": round}),
        )
        .await;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use db::models::{
        issue_found::{IssueCategory, IssueSeverity},
        project::DEFAULT_TEST_PATHS,
    };
    use serde_json::json;
    use sqlx::types::Json;

    use super::*;

    fn defaults() -> Vec<String> {
        DEFAULT_TEST_PATHS.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn default_test_paths_cover_the_usual_layouts_and_nothing_else() {
        let is_test = test_path_matcher(&defaults());
        for path in [
            "tests/api.rs",
            "crates/db/tests/models.rs",
            "web/src/__tests__/App.tsx",
            "web/src/cart.test.ts",
            "web/src/cart.spec.tsx",
            "pkg/cart_test.go",
            "app/test_cart.py",
            "src/test/java/CartTest.java",
            "e2e/checkout.ts",
            "./tests/x.rs",
        ] {
            assert!(is_test(path.trim_start_matches("./")), "{path}");
        }
        for path in [
            "src/lib.rs",
            "src/mytests/x.rs",
            "web/src/cart.ts",
            "testing.md",
            "Cargo.toml",
            "src/attest.rs",
        ] {
            assert!(!is_test(path), "{path}");
        }
    }

    #[test]
    fn custom_test_paths_replace_the_defaults() {
        let is_test = test_path_matcher(&["qa/**".to_string(), "[".to_string()]);
        assert!(is_test("qa/smoke.sh"));
        assert!(!is_test("tests/x.rs"));
    }

    fn issue(status: IssueFoundStatus, description: &str) -> IssueFound {
        IssueFound {
            id: Uuid::nil(),
            issue_id: Uuid::nil(),
            run_id: None,
            checkpoint: Some(0),
            file: Some("shop/src/cart.rs".into()),
            line: Some(7),
            severity: IssueSeverity::High,
            category: IssueCategory::EdgeCase,
            description: description.into(),
            status,
            fixed_round: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn checkpoint() -> OrchestrationCheckpoint {
        OrchestrationCheckpoint {
            id: Uuid::nil(),
            issue_id: Uuid::nil(),
            position: 0,
            what: "Totals are right".into(),
            after_subtasks: Json(vec!["1".into()]),
            status: CheckpointStatus::Testing,
            round: 1,
            tester_workspace_id: None,
            fixer_workspace_id: None,
            execution_process_id: None,
            conflict: None,
            rejected_files: None,
            error: None,
            started_at: None,
            finished_at: None,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn checkpoint_md_lists_paths_earlier_issues_and_the_format() {
        let plan = PlanDocument::from_value(json!({
            "subtasks": [{"id": "1", "title": "Cart"}, {"id": "2", "title": "Mail"}],
            "acceptance_criteria": ["Empty carts total 0"],
            "test_strategy": "Unit tests for the cart."
        }))
        .unwrap();
        let md = render_checkpoint(
            &plan,
            1,
            &checkpoint(),
            &["**/tests/**".into()],
            &[
                issue(IssueFoundStatus::Open, "Empty cart panics"),
                issue(IssueFoundStatus::Wontfix, "Rounding of cents"),
                issue(IssueFoundStatus::Fixed, "Old one"),
            ],
        );
        assert!(md.starts_with("# Checkpoint 1: Totals are right\n"));
        assert!(md.contains("- 1: Cart\n") && !md.contains("2: Mail"));
        assert!(md.contains("- Empty carts total 0"));
        assert!(md.contains("Unit tests for the cart."));
        assert!(md.contains("- `**/tests/**`"));
        assert!(md.contains("high · edge case · shop/src/cart.rs:7: Empty cart panics"));
        assert!(
            md.contains("do not report these)\n\n- [00000000-0000-0000-0000-000000000000] high")
        );
        assert!(!md.contains("Old one"));
        assert!(md.contains("/kanbain/out/issues.json"));
    }

    #[test]
    fn prompts_name_their_context() {
        let t = tester_task("PROJ-1", 2, "Totals", 1, false);
        assert!(t.contains("Test checkpoint 2 of ticket PROJ-1: \"Totals\". This is a re-test"));
        assert!(t.contains("checkpoint.md") && t.contains("issues.json"));
        assert!(t.contains("`PROJ-1: Tests for checkpoint 2`"));
        assert!(!t.contains("conflict.md"));
        let f = fixer_task("PROJ-1", 2, "Totals", 2, true);
        assert!(f.contains("fix round 2 of 3"));
        assert!(f.contains("issues.md") && f.contains("conflict.md"));
        let md = render_issues(2, "Totals", &[issue(IssueFoundStatus::Open, "Boom")]);
        assert!(md.contains("checkpoint 2: Totals") && md.contains("Boom"));
    }

    #[test]
    fn tester_report_needs_issues_json() {
        let mut files = serde_json::Map::new();
        assert!(matches!(tester_report(&files), TesterReport::Missing));
        files.insert("issues.json".into(), json!("[]"));
        assert!(matches!(tester_report(&files), TesterReport::Issues(v) if v.is_empty()));
        files.insert("issues.json".into(), json!("{"));
        assert!(matches!(tester_report(&files), TesterReport::Invalid(_)));
    }
}
