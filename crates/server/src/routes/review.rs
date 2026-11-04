//! Review and merge (Phase 7, D26): what a coded ticket did (diff stats,
//! changelog, issues found, cost), merging it into its target with an
//! optional push, sending it back to the coders or the planner, and the
//! changelog exports (D11).

use std::path::Path as FsPath;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Json as ResponseJson, Response},
    routing::{get, post},
};
use db::models::{
    changelog_entry::{ChangelogEntry, ChangelogKind, CommitRef, CreateChangelogEntry},
    conflict_resolution::ConflictResolution,
    issue::Issue,
    issue_event::{IssueEvent, IssueEventType},
    issue_found::{IssueFound, IssueFoundStatus},
    merge::Merge,
    orchestration::{OrchestrationStatus, TicketOrchestration},
    plan::{IssuePlanning, Plan},
    project::Project,
    project_status::{ProjectStatus, ProjectStatusKind},
    repo::Repo,
    workspace::Workspace,
    workspace_repo::WorkspaceRepo,
};
use deployment::Deployment;
use git::GitCli;
use local_deployment::{
    changelog::{self, TicketChangelog},
    planning::move_to_kind,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use services::services::container::ContainerService;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError, routes::workspaces::git::merge_repo_into_target};

/// Files listed per repo at most; the full diff is in the workspace.
const MAX_FILES: usize = 300;

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReviewFile {
    pub path: String,
    /// `None` for binary files.
    pub added: Option<usize>,
    pub removed: Option<usize>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReviewRepo {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub target_branch: String,
    pub is_target_remote: bool,
    pub commits_ahead: usize,
    pub commits_behind: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub files_changed: usize,
    /// The first [`MAX_FILES`] changed files.
    pub files: Vec<ReviewFile>,
    /// The ticket worktree is mid-rebase/merge or has unmerged files.
    pub has_conflicts: bool,
    pub conflict_resolution: Option<ConflictResolution>,
    /// The latest merge commit into the target, if it was merged.
    pub merged_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReviewCost {
    /// Agent runs (planner, coders, testers, fixers, manual).
    pub runs_usd: f64,
    /// Headless jobs (intake, history, scribe).
    pub jobs_usd: f64,
    pub total_usd: f64,
    pub budget_usd: Option<f64>,
}

/// What the review tab shows.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ReviewView {
    pub workspace_id: Option<Uuid>,
    pub branch: Option<String>,
    #[ts(type = "number | null")]
    pub plan_version: Option<i64>,
    pub acceptance_criteria: Vec<String>,
    pub coding_status: Option<OrchestrationStatus>,
    pub repos: Vec<ReviewRepo>,
    pub changelog: Vec<ChangelogEntry>,
    pub issues_found: Vec<IssueFound>,
    pub cost: ReviewCost,
    /// Everything on the branch is in the target already.
    pub merged: bool,
    /// Why Merge is off (besides `merged`); empty when it may merge.
    pub blockers: Vec<String>,
    /// Worth knowing before merging, but not stopping it.
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
pub struct MergeTicketRequest {
    /// Push the target branch to its remote afterwards.
    #[serde(default)]
    pub push: bool,
}

#[derive(Debug, Serialize, TS)]
pub struct MergeTicketResponse {
    pub merged: Vec<CommitRef>,
    /// One line per repo whose push failed; the merge itself stands.
    pub push_errors: Vec<String>,
    pub pushed: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangesTarget {
    Coders,
    Planner,
}

#[derive(Debug, Deserialize, TS)]
pub struct RequestChangesRequest {
    pub to: ChangesTarget,
    pub feedback: String,
}

/// The ticket's branch: the orchestrator's ticket workspace, else the
/// planner's, else the newest workspace started from the ticket.
async fn ticket_workspace(
    pool: &sqlx::SqlitePool,
    issue_id: Uuid,
) -> Result<Option<Workspace>, sqlx::Error> {
    let candidates = [
        TicketOrchestration::find(pool, issue_id)
            .await?
            .and_then(|o| o.workspace_id),
        IssuePlanning::find(pool, issue_id)
            .await?
            .and_then(|p| p.workspace_id),
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM workspaces WHERE issue_id = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(issue_id)
        .fetch_optional(pool)
        .await?,
    ];
    for id in candidates.into_iter().flatten() {
        if let Some(ws) = Workspace::find_by_id(pool, id).await? {
            return Ok(Some(ws));
        }
    }
    Ok(None)
}

/// `git diff --numstat target...branch`: what merging would bring in.
fn diff_files(repo_path: &FsPath, target: &str, branch: &str) -> Vec<ReviewFile> {
    GitCli::new()
        .git(
            repo_path,
            ["diff", "--numstat", &format!("{target}...{branch}")],
        )
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?;
            let removed = parts.next()?;
            let path = parts.next()?;
            Some(ReviewFile {
                path: path.to_string(),
                added: added.parse().ok(),
                removed: removed.parse().ok(),
            })
        })
        .collect()
}

async fn build_view(deployment: &DeploymentImpl, issue: &Issue) -> Result<ReviewView, ApiError> {
    let pool = &deployment.db().pool;
    let workspace = ticket_workspace(pool, issue.id).await?;
    let orchestration = TicketOrchestration::find(pool, issue.id).await?;
    let plan = Plan::latest(pool, issue.id).await?;
    let changelog = ChangelogEntry::find_by_issue(pool, issue.id).await?;
    let issues_found = IssueFound::find_by_issue(pool, issue.id).await?;

    let runs_usd =
        db::models::execution_process::ExecutionProcess::total_cost_for_issue(pool, issue.id)
            .await?;
    let jobs_usd: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM agent_jobs WHERE issue_id = ?",
    )
    .bind(issue.id)
    .fetch_one(pool)
    .await?;
    let budget_usd = match issue.budget_usd {
        Some(b) => Some(b),
        None => deployment.config().read().await.sandbox.ticket_budget_usd,
    };

    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let mut merged = false;
    let mut repos = Vec::new();
    if let Some(ws) = &workspace {
        // Only look at a worktree that is there: a GET must not recreate
        // the worktree of a merged, archived workspace. Merge makes sure
        // it exists itself.
        let worktree_root = ws
            .container_ref
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists());
        let workspace_repos = WorkspaceRepo::find_by_workspace_id(pool, ws.id).await?;
        let mut resolutions: std::collections::HashMap<Uuid, ConflictResolution> =
            ConflictResolution::latest_for_workspace(pool, ws.id)
                .await?
                .into_iter()
                .map(|r| (r.repo_id, r))
                .collect();
        for wr in workspace_repos {
            let Some(repo) = Repo::find_by_id(pool, wr.repo_id).await? else {
                continue;
            };
            let git = deployment.git();
            let is_target_remote = git
                .is_remote_branch(&repo.path, &wr.target_branch)
                .unwrap_or(false);
            let (ahead, behind) = git
                .get_branch_status(&repo.path, &ws.branch, &wr.target_branch)
                .unwrap_or((0, 0));
            let path = repo.path.clone();
            let (target, branch) = (wr.target_branch.clone(), ws.branch.clone());
            let files = tokio::task::spawn_blocking(move || diff_files(&path, &target, &branch))
                .await
                .unwrap_or_default();
            let (mut conflicted, mut op) = (false, false);
            if let Some(root) = &worktree_root {
                let worktree = root.join(&repo.name);
                conflicted = !git
                    .get_conflicted_files(&worktree)
                    .unwrap_or_default()
                    .is_empty();
                op = git.detect_conflict_op(&worktree).ok().flatten().is_some();
                let (uncommitted, _) = git.get_worktree_change_counts(&worktree).unwrap_or((0, 0));
                if uncommitted > 0 {
                    warnings.push(format!(
                        "{} has {uncommitted} uncommitted change{} that won't be merged",
                        repo.name,
                        if uncommitted == 1 { "" } else { "s" }
                    ));
                }
            }
            let merged_commit = Merge::find_by_workspace_and_repo_id(pool, ws.id, repo.id)
                .await?
                .into_iter()
                .find_map(|m| match m {
                    Merge::Direct(d) => Some(d.merge_commit),
                    Merge::Pr(_) => None,
                });
            let resolution = resolutions.remove(&repo.id);

            if is_target_remote {
                blockers.push(format!(
                    "{} targets the remote branch {}; open a pull request instead",
                    repo.name, wr.target_branch
                ));
            }
            if conflicted || op {
                blockers.push(format!(
                    "{} is in the middle of a rebase or merge; resolve or abort it",
                    repo.name
                ));
            } else if resolution.as_ref().is_some_and(|r| r.stage.is_active()) {
                blockers.push(format!(
                    "The agent is still rebasing {} onto {}",
                    repo.name, wr.target_branch
                ));
            } else if behind > 0 && ahead > 0 {
                blockers.push(format!(
                    "{} is {behind} commit{} behind {}; rebase first",
                    repo.name,
                    if behind == 1 { "" } else { "s" },
                    wr.target_branch
                ));
            }

            repos.push(ReviewRepo {
                repo_id: repo.id,
                repo_name: repo.name.clone(),
                target_branch: wr.target_branch.clone(),
                is_target_remote,
                commits_ahead: ahead,
                commits_behind: behind,
                lines_added: files.iter().filter_map(|f| f.added).sum(),
                lines_removed: files.iter().filter_map(|f| f.removed).sum(),
                files_changed: files.len(),
                files: files.into_iter().take(MAX_FILES).collect(),
                has_conflicts: conflicted || op,
                conflict_resolution: resolution,
                merged_commit,
            });
        }
        if repos.iter().all(|r| r.commits_ahead == 0) {
            if repos.iter().any(|r| r.merged_commit.is_some()) {
                merged = true;
            } else {
                blockers.push("Nothing to merge: the branch has no commits".to_string());
            }
        }
    } else {
        blockers.push("This ticket has no branch to merge".to_string());
    }

    let coding_status = orchestration.as_ref().map(|o| o.status);
    if coding_status == Some(OrchestrationStatus::Running) {
        blockers.push("Coding is still running".to_string());
    }
    let open = issues_found
        .iter()
        .filter(|i| i.status == IssueFoundStatus::Open)
        .count();
    if open > 0 {
        warnings.push(format!(
            "{open} issue{} found by the tester {} still open",
            if open == 1 { "" } else { "s" },
            if open == 1 { "is" } else { "are" }
        ));
    }
    let total = runs_usd + jobs_usd;
    if let Some(budget) = budget_usd
        && total > budget
    {
        warnings.push(format!("Spent ${total:.2} of a ${budget:.2} budget"));
    }

    Ok(ReviewView {
        workspace_id: workspace.as_ref().map(|w| w.id),
        branch: workspace.as_ref().map(|w| w.branch.clone()),
        plan_version: plan.as_ref().map(|p| p.version),
        acceptance_criteria: plan
            .map(|p| p.plan.0.acceptance_criteria.clone())
            .unwrap_or_default(),
        coding_status,
        repos,
        changelog,
        issues_found,
        cost: ReviewCost {
            runs_usd,
            jobs_usd,
            total_usd: total,
            budget_usd,
        },
        merged,
        blockers,
        warnings,
    })
}

async fn find_issue(deployment: &DeploymentImpl, issue_id: Uuid) -> Result<Issue, ApiError> {
    Issue::find_by_id(&deployment.db().pool, issue_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))
}

async fn get_review(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ReviewView>>, ApiError> {
    let issue = find_issue(&deployment, issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        build_view(&deployment, &issue).await?,
    )))
}

/// The remote a branch pushes to: its configured one, else `origin`.
fn push_target(repo_path: &FsPath, branch: &str) -> Result<(), String> {
    let git = GitCli::new();
    let remote = git
        .git(
            repo_path,
            ["config", "--get", &format!("branch.{branch}.remote")],
        )
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "origin".to_string());
    git.git(repo_path, ["push", &remote, branch])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Merges every repo of the ticket branch into its target (local squash
/// merge, D26), records it in the changelog, moves the ticket to Done and
/// re-indexes the merged code (D45, E17, E19).
async fn merge_ticket(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<MergeTicketRequest>,
) -> Result<ResponseJson<ApiResponse<MergeTicketResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let issue = find_issue(&deployment, issue_id).await?;
    let view = build_view(&deployment, &issue).await?;
    if view.merged {
        return Err(ApiError::BadRequest(
            "Already merged; nothing new on the branch".into(),
        ));
    }
    if !view.blockers.is_empty() {
        return Err(ApiError::BadRequest(view.blockers.join("; ")));
    }
    let workspace = ticket_workspace(pool, issue_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("This ticket has no branch to merge".into()))?;

    let mut merged = Vec::new();
    let mut messages = Vec::new();
    let mut merged_repos = Vec::new();
    for review_repo in view.repos.iter().filter(|r| r.commits_ahead > 0) {
        let wr =
            WorkspaceRepo::find_by_workspace_and_repo_id(pool, workspace.id, review_repo.repo_id)
                .await?
                .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
        let repo = Repo::find_by_id(pool, review_repo.repo_id)
            .await?
            .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
        let (sha, message) = merge_repo_into_target(&deployment, &workspace, &wr, &repo).await?;
        merged.push(CommitRef {
            repo: repo.name.clone(),
            sha,
        });
        messages.push(format!("### {}\n\n{}", repo.name, message.trim()));
        merged_repos.push((repo, wr.target_branch));
    }

    let mut pushed = Vec::new();
    let mut push_errors = Vec::new();
    if request.push {
        for (repo, target) in &merged_repos {
            let (path, branch) = (repo.path.clone(), target.clone());
            match tokio::task::spawn_blocking(move || push_target(&path, &branch)).await {
                Ok(Ok(())) => pushed.push(repo.name.clone()),
                Ok(Err(e)) => push_errors.push(format!("{}: {e}", repo.name)),
                Err(e) => push_errors.push(format!("{}: {e}", repo.name)),
            }
        }
    }

    let files: Vec<String> = view
        .repos
        .iter()
        .filter(|r| r.commits_ahead > 0)
        .flat_map(|r| {
            r.files
                .iter()
                .map(move |f| format!("{}/{}", r.repo_name, f.path))
        })
        .collect();
    let targets: Vec<&str> = merged_repos.iter().map(|(_, t)| t.as_str()).collect();
    let mut targets_dedup = targets.clone();
    targets_dedup.dedup();
    let entry = ChangelogEntry::create(
        pool,
        CreateChangelogEntry {
            issue_id,
            run_id: None,
            job_id: None,
            kind: ChangelogKind::Merge,
            title: &format!("Merged into {}", targets_dedup.join(", ")),
            summary: &messages.join("\n\n"),
            files: &files,
            commit_refs: &merged,
            issues_fixed: &[],
            issues_found: &[],
            scribe_error: None,
        },
    )
    .await?;
    IssueEvent::create(
        pool,
        issue_id,
        IssueEventType::Merged,
        "user",
        &json!({"repos": merged, "pushed": pushed, "push_errors": push_errors,
                "entry": entry.as_ref().map(|e| e.id)}),
        None,
    )
    .await?;

    if let Some(issue) = Issue::find_by_id(pool, issue_id).await? {
        move_to_kind(pool, &issue, ProjectStatusKind::Done).await;
    }
    if !workspace.pinned
        && let Err(e) = deployment.container().archive_workspace(workspace.id).await
    {
        tracing::warn!(
            "Archiving the ticket workspace {} failed: {e}",
            workspace.id
        );
    }
    // Main moved: re-index the merged repos, drop the ticket's overlay
    // (Done) and link the merged commits to the ticket.
    if let Some(search) = deployment.search() {
        for (repo, _) in &merged_repos {
            search.enqueue_repo(issue.project_id, repo.id).await;
        }
        search.enqueue_issue(issue_id).await;
    }
    deployment.history().spawn_check(issue_id);

    Ok(ResponseJson(ApiResponse::success(MergeTicketResponse {
        merged,
        push_errors,
        pushed,
    })))
}

async fn request_changes(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<RequestChangesRequest>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    find_issue(&deployment, issue_id).await?;
    match request.to {
        ChangesTarget::Coders => {
            deployment
                .orchestrator()
                .request_changes(issue_id, &request.feedback)
                .await?;
        }
        ChangesTarget::Planner => {
            deployment
                .planning()
                .replan(issue_id, &request.feedback)
                .await?;
            IssueEvent::create(
                &deployment.db().pool,
                issue_id,
                IssueEventType::ChangesRequested,
                "user",
                &json!({"to": "planner", "feedback": request.feedback.trim()}),
                None,
            )
            .await?;
        }
    }
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn ticket_changelog_parts(
    pool: &sqlx::SqlitePool,
    issue: &Issue,
    project: &Project,
) -> Result<(String, String, Vec<ChangelogEntry>, Vec<IssueFound>), sqlx::Error> {
    let id = issue.display_id(&project.id_prefix);
    let status = ProjectStatus::find_by_id(pool, issue.status_id)
        .await?
        .map(|s| s.name)
        .unwrap_or_default();
    let entries = ChangelogEntry::find_by_issue(pool, issue.id).await?;
    let issues = IssueFound::find_by_issue(pool, issue.id).await?;
    Ok((id, status, entries, issues))
}

fn markdown_download(body: String, file_name: &str) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "text/markdown; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        body,
    )
        .into_response()
}

async fn export_ticket_changelog(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<Response, ApiError> {
    let pool = &deployment.db().pool;
    let issue = find_issue(&deployment, issue_id).await?;
    let project = Project::find_by_id(pool, issue.project_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let (id, status, entries, issues) = ticket_changelog_parts(pool, &issue, &project).await?;
    let body = changelog::render_ticket(&TicketChangelog {
        id: &id,
        issue: &issue,
        status: &status,
        entries: &entries,
        issues_found: &issues,
    });
    Ok(markdown_download(body, &changelog::file_name(&id)))
}

async fn export_project_changelog(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<Response, ApiError> {
    let pool = &deployment.db().pool;
    let project = Project::find_by_id(pool, project_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let issues = Issue::find_by_project(pool, project_id).await?;
    let mut parts = Vec::new();
    for issue in &issues {
        parts.push(ticket_changelog_parts(pool, issue, &project).await?);
    }
    let tickets: Vec<TicketChangelog<'_>> = issues
        .iter()
        .zip(&parts)
        .map(|(issue, (id, status, entries, found))| TicketChangelog {
            id,
            issue,
            status,
            entries,
            issues_found: found,
        })
        .collect();
    let body = changelog::render_project(&project.name, &tickets);
    Ok(markdown_download(
        body,
        &changelog::file_name(&project.name),
    ))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/issues/{issue_id}/review", get(get_review))
        .route("/issues/{issue_id}/merge", post(merge_ticket))
        .route("/issues/{issue_id}/request-changes", post(request_changes))
        .route(
            "/issues/{issue_id}/changelog.md",
            get(export_ticket_changelog),
        )
        .route(
            "/projects/{project_id}/changelog.md",
            get(export_project_changelog),
        )
}
