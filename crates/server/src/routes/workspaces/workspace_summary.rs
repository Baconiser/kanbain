use std::collections::HashMap;

use axum::{Json, extract::State, response::Json as ResponseJson};
use db::models::{
    coding_agent_turn::CodingAgentTurn,
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    merge::MergeStatus,
    pull_request::PullRequest,
    workspace::{Workspace, WorkspaceTicketRole},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

/// Request for fetching workspace summaries
#[derive(Debug, Deserialize, Serialize, TS)]
pub struct WorkspaceSummaryRequest {
    pub archived: bool,
}

/// Summary info for a single workspace
#[derive(Debug, Serialize, TS)]
pub struct WorkspaceSummary {
    pub workspace_id: Uuid,
    /// Session ID of the latest execution process
    pub latest_session_id: Option<Uuid>,
    /// Is a tool approval currently pending?
    pub has_pending_approval: bool,
    /// Number of files with changes
    pub files_changed: Option<usize>,
    /// Total lines added across all files
    pub lines_added: Option<usize>,
    /// Total lines removed across all files
    pub lines_removed: Option<usize>,
    /// When the latest execution process completed
    #[ts(optional)]
    pub latest_process_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Status of the latest execution process
    pub latest_process_status: Option<ExecutionProcessStatus>,
    /// Is a dev server currently running?
    pub has_running_dev_server: bool,
    /// Does this workspace have unseen coding agent turns?
    pub has_unseen_turns: bool,
    /// PR status for this workspace (if any PR exists)
    pub pr_status: Option<MergeStatus>,
    /// PR number for this workspace (if any PR exists)
    pub pr_number: Option<i64>,
    /// PR URL for this workspace (if any PR exists)
    pub pr_url: Option<String>,
    /// The main ticket this workspace works for
    pub ticket_id: Option<Uuid>,
    /// Number of that ticket; its accent colour derives from it
    #[ts(type = "number | null")]
    pub ticket_number: Option<i64>,
    /// Orchestrator subtask run in this workspace, for Retry
    pub subtask_id: Option<Uuid>,
    /// Orchestrator checkpoint tested or fixed in this workspace, for Retry
    pub checkpoint_id: Option<Uuid>,
    /// The one thing the user should do next
    pub next_action: WorkspaceNextAction,
}

/// What a workspace waits for, most urgent first. The sidebar shows one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceNextAction {
    /// A tool call waits for approval.
    Approve,
    /// An agent is running; nothing to do.
    Working,
    /// The last run failed or its subtask/checkpoint stopped.
    Retry,
    /// The ticket's own workspace waits for review and merge.
    Review,
    /// An agent finished and nobody looked yet.
    Check,
    /// Its work landed or its ticket is done; only archiving is left.
    Archive,
    None,
}

/// What the next action is decided from.
#[derive(Debug, Default)]
struct NextActionInput<'a> {
    pending_approval: bool,
    running: bool,
    errored: bool,
    unseen: bool,
    pr_merged: bool,
    role: Option<&'a WorkspaceTicketRole>,
}

fn next_action(input: &NextActionInput) -> WorkspaceNextAction {
    let role = input.role;
    let status = |f: fn(&WorkspaceTicketRole) -> &Option<String>| {
        role.and_then(|r| f(r).as_deref()).unwrap_or_default()
    };
    let subtask = status(|r| &r.subtask_status);
    let checkpoint = status(|r| &r.checkpoint_status);
    let ticket = status(|r| &r.ticket_status);
    let orchestrated = !subtask.is_empty() || !checkpoint.is_empty();

    if input.pending_approval {
        return WorkspaceNextAction::Approve;
    }
    if input.running {
        return WorkspaceNextAction::Working;
    }
    if input.pr_merged
        || role.is_some_and(|r| r.merged)
        || subtask == "merged"
        || checkpoint == "passed"
        || ticket == "done"
    {
        return WorkspaceNextAction::Archive;
    }
    if matches!(subtask, "failed" | "stopped")
        || matches!(checkpoint, "failed" | "stopped")
        || (!orchestrated && input.errored)
    {
        return WorkspaceNextAction::Retry;
    }
    if !orchestrated && ticket == "review" {
        return WorkspaceNextAction::Review;
    }
    if input.unseen {
        return WorkspaceNextAction::Check;
    }
    WorkspaceNextAction::None
}

/// Response containing summaries for requested workspaces
#[derive(Debug, Serialize, TS)]
pub struct WorkspaceSummaryResponse {
    pub summaries: Vec<WorkspaceSummary>,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct DiffStats {
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// Fetch summary information for workspaces filtered by archived status.
/// This endpoint returns data that cannot be efficiently included in the streaming endpoint.
#[axum::debug_handler]
pub async fn get_workspace_summaries(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<WorkspaceSummaryRequest>,
) -> Result<ResponseJson<ApiResponse<WorkspaceSummaryResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let archived = request.archived;

    // 1. Fetch all workspaces with the given archived status
    let with_status = Workspace::find_all_with_status(pool, Some(archived), None).await?;
    let running: HashMap<Uuid, (bool, bool)> = with_status
        .iter()
        .map(|ws| (ws.workspace.id, (ws.is_running, ws.is_errored)))
        .collect();
    let workspaces: Vec<Workspace> = with_status.into_iter().map(|ws| ws.workspace).collect();

    if workspaces.is_empty() {
        return Ok(ResponseJson(ApiResponse::success(
            WorkspaceSummaryResponse { summaries: vec![] },
        )));
    }

    // 2. Fetch latest process info for workspaces with this archived status
    let latest_processes = ExecutionProcess::find_latest_for_workspaces(pool, archived).await?;

    // 3. Check which workspaces have running dev servers
    let dev_server_workspaces =
        ExecutionProcess::find_workspaces_with_running_dev_servers(pool, archived).await?;

    // 4. Check pending approvals for running processes
    let running_ep_ids: Vec<_> = latest_processes
        .values()
        .filter(|info| info.status == ExecutionProcessStatus::Running)
        .map(|info| info.execution_process_id)
        .collect();
    let pending_approval_eps = deployment
        .approvals()
        .get_pending_execution_process_ids(&running_ep_ids);

    // 5. Check which workspaces have unseen coding agent turns
    let unseen_workspaces = CodingAgentTurn::find_workspaces_with_unseen(pool, archived).await?;

    // 6. Get PR status for each workspace
    let pr_statuses = PullRequest::get_latest_for_workspaces(pool, archived).await?;

    // 7. Ticket and orchestrator role of each workspace
    let roles: HashMap<Uuid, WorkspaceTicketRole> = Workspace::ticket_roles(pool, archived)
        .await?
        .into_iter()
        .map(|r| (r.workspace_id, r))
        .collect();

    // 8. Compute diff stats for each workspace (in parallel)
    let diff_futures: Vec<_> = workspaces
        .iter()
        .map(|ws| {
            let workspace = ws.clone();
            let deployment = deployment.clone();
            async move {
                if workspace.container_ref.is_some() {
                    compute_workspace_diff_stats(&deployment, &workspace)
                        .await
                        .map(|stats| (workspace.id, stats))
                } else {
                    None
                }
            }
        })
        .collect();

    let diff_results: Vec<Option<(Uuid, DiffStats)>> =
        futures_util::future::join_all(diff_futures).await;
    let diff_stats: HashMap<Uuid, DiffStats> = diff_results.into_iter().flatten().collect();

    // 9. Assemble response
    let summaries: Vec<WorkspaceSummary> = workspaces
        .iter()
        .map(|ws| {
            let id = ws.id;
            let latest = latest_processes.get(&id);
            let has_pending = latest
                .map(|p| pending_approval_eps.contains(&p.execution_process_id))
                .unwrap_or(false);
            let stats = diff_stats.get(&id);
            let role = roles.get(&id);
            let pr = pr_statuses.get(&id);
            let (is_running, is_errored) = running.get(&id).copied().unwrap_or_default();
            let unseen = unseen_workspaces.contains(&id);

            WorkspaceSummary {
                workspace_id: id,
                latest_session_id: latest.map(|p| p.session_id),
                has_pending_approval: has_pending,
                files_changed: stats.map(|s| s.files_changed),
                lines_added: stats.map(|s| s.lines_added),
                lines_removed: stats.map(|s| s.lines_removed),
                latest_process_completed_at: latest.and_then(|p| p.completed_at),
                latest_process_status: latest.map(|p| p.status.clone()),
                has_running_dev_server: dev_server_workspaces.contains(&id),
                has_unseen_turns: unseen,
                pr_status: pr.map(|pr| pr.pr_status.clone()),
                pr_number: pr.map(|pr| pr.pr_number),
                pr_url: pr.map(|pr| pr.pr_url.clone()),
                ticket_id: role.and_then(|r| r.ticket_id),
                ticket_number: role.and_then(|r| r.ticket_number),
                subtask_id: role.and_then(|r| r.subtask_id),
                checkpoint_id: role.and_then(|r| r.checkpoint_id),
                next_action: next_action(&NextActionInput {
                    pending_approval: has_pending,
                    running: is_running,
                    errored: is_errored,
                    unseen,
                    pr_merged: pr.is_some_and(|pr| matches!(pr.pr_status, MergeStatus::Merged)),
                    role,
                }),
            }
        })
        .collect();

    Ok(ResponseJson(ApiResponse::success(
        WorkspaceSummaryResponse { summaries },
    )))
}

/// Compute diff stats for a workspace.
pub async fn compute_workspace_diff_stats(
    deployment: &DeploymentImpl,
    workspace: &Workspace,
) -> Option<DiffStats> {
    let stats = services::services::diff_stream::compute_diff_stats(
        &deployment.db().pool,
        deployment.git(),
        workspace,
    )
    .await?;

    Some(DiffStats {
        files_changed: stats.files_changed,
        lines_added: stats.lines_added,
        lines_removed: stats.lines_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(
        subtask: Option<&str>,
        checkpoint: Option<&str>,
        ticket: Option<&str>,
    ) -> WorkspaceTicketRole {
        WorkspaceTicketRole {
            workspace_id: Uuid::nil(),
            ticket_id: Some(Uuid::nil()),
            ticket_number: Some(1),
            ticket_status: ticket.map(str::to_string),
            subtask_id: subtask.map(|_| Uuid::nil()),
            subtask_status: subtask.map(str::to_string),
            checkpoint_id: checkpoint.map(|_| Uuid::nil()),
            checkpoint_status: checkpoint.map(str::to_string),
            merged: false,
        }
    }

    fn action(
        role: &WorkspaceTicketRole,
        f: impl FnOnce(&mut NextActionInput),
    ) -> WorkspaceNextAction {
        let mut input = NextActionInput {
            role: Some(role),
            ..Default::default()
        };
        f(&mut input);
        next_action(&input)
    }

    #[test]
    fn approval_and_running_beat_everything() {
        let done = role(None, Some("passed"), Some("done"));
        assert_eq!(
            action(&done, |i| {
                i.pending_approval = true;
                i.running = true;
            }),
            WorkspaceNextAction::Approve
        );
        assert_eq!(
            action(&done, |i| i.running = true),
            WorkspaceNextAction::Working
        );
    }

    #[test]
    fn landed_work_and_done_tickets_only_need_archiving() {
        let unseen = |i: &mut NextActionInput| i.unseen = true;
        for r in [
            role(None, Some("passed"), Some("review")),
            role(Some("merged"), None, Some("in_progress")),
            role(None, Some("failed"), Some("done")),
            role(None, None, Some("done")),
        ] {
            assert_eq!(action(&r, unseen), WorkspaceNextAction::Archive, "{r:?}");
        }
        let mut merged = role(None, None, Some("review"));
        merged.merged = true;
        assert_eq!(action(&merged, |_| {}), WorkspaceNextAction::Archive);
    }

    #[test]
    fn failures_ask_for_a_retry_unless_the_orchestrator_will_rerun() {
        let failed = role(None, Some("failed"), Some("testing"));
        assert_eq!(action(&failed, |_| {}), WorkspaceNextAction::Retry);
        let stopped = role(Some("stopped"), None, Some("in_progress"));
        assert_eq!(action(&stopped, |_| {}), WorkspaceNextAction::Retry);
        // A pending subtask whose last run failed is re-run on its own.
        let pending = role(Some("pending"), None, Some("in_progress"));
        assert_eq!(
            action(&pending, |i| i.errored = true),
            WorkspaceNextAction::None
        );
        let manual = role(None, None, Some("in_progress"));
        assert_eq!(
            action(&manual, |i| i.errored = true),
            WorkspaceNextAction::Retry
        );
    }

    #[test]
    fn the_ticket_workspace_waits_for_review_and_unseen_runs_get_checked() {
        let ticket = role(None, None, Some("review"));
        assert_eq!(
            action(&ticket, |i| i.unseen = true),
            WorkspaceNextAction::Review
        );
        let tester = role(None, Some("testing"), Some("review"));
        assert_eq!(
            action(&tester, |i| i.unseen = true),
            WorkspaceNextAction::Check
        );
        assert_eq!(action(&tester, |_| {}), WorkspaceNextAction::None);
        assert_eq!(
            next_action(&NextActionInput::default()),
            WorkspaceNextAction::None
        );
    }
}
