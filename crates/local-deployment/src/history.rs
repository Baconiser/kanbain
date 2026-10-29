//! The git-history check (D18) and ticket ↔ code links (E19).
//!
//! When a ticket is created, its External ID changes, or a run on it starts
//! or ends, every repo of its project is searched for commits mentioning
//! the ticket's id and for branches named after it. The commits and the
//! files they changed are stored, and appear on the timeline at their commit
//! time. Commits not made by KanbAIn's agents are prior work: with the
//! user's permission (per ticket, or "always allow" per project) the history
//! agent summarizes them into `prior_work.md`.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use db::{
    DBService,
    models::{
        agent_job::JOB_KIND_HISTORY,
        issue::Issue,
        issue_event::{IssueEvent, IssueEventType},
        issue_history::{
            HistoryPermission, IssueHistory, NewCommitFile, NewTicketCommit, TicketCommit,
        },
        project::{Project, ProjectRepo},
        repo::Repo,
    },
};
use executors::adapters::AgentRole;
use git::history::{HistoryCommit, commit_patch, find_ticket_commits};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::jobs::{AgentJobs, JobRequest};

/// Most commits recorded per repo.
const MAX_COMMITS_PER_REPO: usize = 200;
/// Commit messages plus diffs sent to the history agent (D18: about 200 KB).
const MAX_CONTEXT_BYTES: usize = 200 * 1024;
/// One commit's patch larger than this is reduced to its file list.
const MAX_PATCH_BYTES: usize = 48 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Ticket not found")]
    NotFound,
}

#[derive(Clone)]
pub struct HistoryService {
    db: DBService,
    jobs: AgentJobs,
    /// Tickets with a check in progress; a second request waits for none.
    checking: Arc<Mutex<HashSet<Uuid>>>,
}

/// One repo's commits for a ticket.
struct RepoCommits {
    repo: Repo,
    commits: Vec<HistoryCommit>,
}

/// Commits the history agent may read: everything not committed by
/// KanbAIn's own agents.
fn is_prior_work(commit: &HistoryCommit) -> bool {
    commit.author != sandbox::AGENT_GIT_NAME
}

fn hash_shas(shas: &mut [&str]) -> String {
    shas.sort_unstable();
    let mut hasher = Sha256::new();
    for sha in shas.iter() {
        hasher.update(sha.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// The history agent's task: the ticket, the rules and the commits with
/// their diffs, capped at [`MAX_CONTEXT_BYTES`].
pub fn history_task(id: &str, title: &str, commits: &[(String, String)]) -> String {
    let mut task = format!(
        "Earlier commits mention the ticket {id} (\"{title}\"). Summarize that prior work \
         for the engineer who plans the ticket now.\n\n\
         Answer with Markdown only, in these sections:\n\
         ## What was done\n## What was left open\n## What the planner should know\n\
         ## Files involved\n\n\
         Be concrete: name files, functions and decisions. Say when the commits don't tell. \
         Do not invent anything the commits don't show.\n"
    );
    for (header, body) in commits {
        task.push_str(&format!(
            "\n<commit {header}>\n{}\n</commit>\n",
            body.trim()
        ));
    }
    task
}

impl HistoryService {
    pub fn new(db: DBService, jobs: AgentJobs) -> Self {
        Self {
            db,
            jobs,
            checking: Arc::default(),
        }
    }

    /// Checks in the background; failures are logged.
    pub fn spawn_check(&self, issue_id: Uuid) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.check(issue_id).await {
                tracing::warn!("History check for ticket {issue_id} failed: {e}");
            }
        });
    }

    async fn scan(&self, project_id: Uuid, id: &str) -> Result<Vec<RepoCommits>, HistoryError> {
        let repos = ProjectRepo::find_repos_for_project(&self.db.pool, project_id).await?;
        let mut found = Vec::new();
        for repo in repos {
            let path = repo.path.clone();
            let search = id.to_string();
            let result = tokio::task::spawn_blocking(move || {
                find_ticket_commits(&path, &search, MAX_COMMITS_PER_REPO)
            })
            .await;
            match result {
                Ok(Ok(commits)) => found.push(RepoCommits { repo, commits }),
                Ok(Err(e)) => tracing::warn!("Searching {} for {id} failed: {e}", repo.name),
                Err(e) => tracing::warn!("Searching {} for {id} failed: {e}", repo.name),
            }
        }
        Ok(found)
    }

    /// Searches the project's repos for the ticket's commits, stores them
    /// with their files, adds new ones to the timeline, and starts the
    /// analysis when it is allowed and the prior work changed.
    pub async fn check(&self, issue_id: Uuid) -> Result<Option<IssueHistory>, HistoryError> {
        if !self.checking.lock().unwrap().insert(issue_id) {
            return Ok(None);
        }
        let result = self.check_inner(issue_id).await;
        self.checking.lock().unwrap().remove(&issue_id);
        result.map(Some)
    }

    async fn check_inner(&self, issue_id: Uuid) -> Result<IssueHistory, HistoryError> {
        let pool = &self.db.pool;
        let issue = Issue::find_by_id(pool, issue_id)
            .await?
            .ok_or(HistoryError::NotFound)?;
        let project = Project::find_by_id(pool, issue.project_id)
            .await?
            .ok_or(HistoryError::NotFound)?;
        let id = issue.display_id(&project.id_prefix);
        let previous = IssueHistory::find(pool, issue_id).await?;

        let found = self.scan(project.id, &id).await?;
        let mut total = 0;
        let mut prior_shas: Vec<&str> = Vec::new();
        for RepoCommits { repo, commits } in &found {
            total += commits.len();
            prior_shas.extend(
                commits
                    .iter()
                    .filter(|c| is_prior_work(c))
                    .map(|c| c.sha.as_str()),
            );
            let rows: Vec<NewTicketCommit> = commits
                .iter()
                .map(|c| NewTicketCommit {
                    sha: c.sha.clone(),
                    author: c.author.clone(),
                    committed_at: c.committed_at,
                    subject: c.subject.clone(),
                    branches: c.branches.clone(),
                    message_match: c.message_match,
                    files: c
                        .files
                        .iter()
                        .map(|f| NewCommitFile {
                            path: f.path.clone(),
                            additions: f.additions.map(i64::from),
                            deletions: f.deletions.map(i64::from),
                            hunks: f
                                .hunks
                                .iter()
                                .map(|(a, b)| (i64::from(*a), i64::from(*b)))
                                .collect(),
                        })
                        .collect(),
                })
                .collect();
            TicketCommit::replace_for_repo(pool, issue_id, repo.id, &rows).await?;

            for commit in commits {
                if IssueEvent::exists_with(
                    pool,
                    issue_id,
                    IssueEventType::PriorCommit,
                    "sha",
                    &commit.sha,
                )
                .await?
                {
                    continue;
                }
                let actor = if is_prior_work(commit) {
                    "system".to_string()
                } else {
                    "agent".to_string()
                };
                IssueEvent::create(
                    pool,
                    issue_id,
                    IssueEventType::PriorCommit,
                    &actor,
                    &json!({
                        "sha": commit.sha,
                        "short_sha": commit.short_sha(),
                        "repo": repo.name,
                        "subject": commit.subject,
                        "author": commit.author,
                        "branches": commit.branches,
                        "files": commit.files.len(),
                    }),
                    Some(commit.committed_at),
                )
                .await?;
            }
        }
        let prior_count = prior_shas.len() as i64;
        let prior_hash = hash_shas(&mut prior_shas);

        let history =
            IssueHistory::record_check(pool, issue_id, &id, total as i64, prior_count, &prior_hash)
                .await?;
        let changed = previous
            .as_ref()
            .is_none_or(|p| p.prior_hash != history.prior_hash || p.searched_id != id);
        if prior_count > 0 && changed {
            IssueEvent::create(
                pool,
                issue_id,
                IssueEventType::HistoryCheck,
                "system",
                &json!({
                    "id": id,
                    "commit_count": prior_count,
                    "repos": found
                        .iter()
                        .filter(|r| !r.commits.is_empty())
                        .map(|r| r.repo.name.clone())
                        .collect::<Vec<_>>(),
                }),
                None,
            )
            .await?;
        }

        let allowed = project.settings.history_always_allow
            || history.permission == HistoryPermission::Allowed;
        if allowed && history.needs_analysis() {
            self.spawn_analysis(issue_id);
        }
        Ok(history)
    }

    /// The user allowed the analysis for this ticket.
    pub async fn allow(&self, issue_id: Uuid) -> Result<(), HistoryError> {
        IssueHistory::set_permission(&self.db.pool, issue_id, HistoryPermission::Allowed).await?;
        let history = IssueHistory::find(&self.db.pool, issue_id)
            .await?
            .ok_or(HistoryError::NotFound)?;
        if history.needs_analysis() || history.prior_work.is_none() {
            self.spawn_analysis(issue_id);
        }
        Ok(())
    }

    pub async fn deny(&self, issue_id: Uuid) -> Result<(), HistoryError> {
        IssueHistory::set_permission(&self.db.pool, issue_id, HistoryPermission::Denied).await?;
        Ok(())
    }

    fn spawn_analysis(&self, issue_id: Uuid) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.analyze(issue_id).await {
                tracing::error!("Prior-work analysis for ticket {issue_id} failed: {e}");
            }
        });
    }

    /// Runs the history agent over the prior-work commits (messages plus
    /// diffs, capped) and stores its summary.
    async fn analyze(&self, issue_id: Uuid) -> Result<(), HistoryError> {
        let pool = &self.db.pool;
        if !IssueHistory::start_analysis(pool, issue_id).await? {
            return Ok(());
        }
        let issue = Issue::find_by_id(pool, issue_id)
            .await?
            .ok_or(HistoryError::NotFound)?;
        let project = Project::find_by_id(pool, issue.project_id)
            .await?
            .ok_or(HistoryError::NotFound)?;
        let id = issue.display_id(&project.id_prefix);

        // Fresh scan, so the diffs come from the repos as they are now.
        let found = self.scan(project.id, &id).await?;
        let mut shas: Vec<&str> = Vec::new();
        let mut sections: Vec<(String, String)> = Vec::new();
        let mut budget = MAX_CONTEXT_BYTES;
        let mut reduced = 0;
        let mut commit_count = 0;
        for RepoCommits { repo, commits } in &found {
            for commit in commits.iter().filter(|c| is_prior_work(c)) {
                commit_count += 1;
                shas.push(&commit.sha);
                let header = format!(
                    "repo=\"{}\" sha=\"{}\" date=\"{}\"",
                    repo.name,
                    commit.short_sha(),
                    commit.committed_at.format("%Y-%m-%d")
                );
                let path = repo.path.clone();
                let sha = commit.sha.clone();
                let max = MAX_PATCH_BYTES.min(budget);
                let patch = tokio::task::spawn_blocking(move || commit_patch(&path, &sha, max))
                    .await
                    .ok()
                    .and_then(Result::ok);
                let body = match patch {
                    Some((text, was_reduced)) if text.len() <= budget => {
                        if was_reduced {
                            reduced += 1;
                        }
                        text
                    }
                    _ => {
                        reduced += 1;
                        let files: Vec<&str> =
                            commit.files.iter().map(|f| f.path.as_str()).collect();
                        format!(
                            "{}\n\n{}\n\nFiles: {}",
                            commit.subject,
                            commit.body,
                            files.join(", ")
                        )
                    }
                };
                budget = budget.saturating_sub(body.len());
                sections.push((header, body));
            }
        }
        let analyzed_hash = hash_shas(&mut shas);
        if sections.is_empty() {
            IssueHistory::finish_analysis(pool, issue_id, None, Err("No prior commits found"))
                .await?;
            return Ok(());
        }

        let task = history_task(&id, &issue.title, &sections);
        let context = vec![("commits.md".to_string(), task.clone())];
        let (job_id, result) = self
            .jobs
            .run(JobRequest {
                kind: JOB_KIND_HISTORY,
                role: AgentRole::History,
                project: Some(project),
                issue_id: Some(issue_id),
                context,
                task,
            })
            .await;
        let job = match job_id {
            Some(id) => db::models::agent_job::AgentJob::find_by_id(pool, id).await?,
            None => None,
        };
        let usage = json!({
            "id": id,
            "commit_count": commit_count,
            "reduced_diffs": reduced,
            "job_id": job_id,
            "agent": job.as_ref().and_then(|j| j.agent_def_id.clone()),
            "model": job.as_ref().and_then(|j| j.model.clone()),
            "tokens_in": job.as_ref().map(|j| j.tokens_in),
            "tokens_out": job.as_ref().map(|j| j.tokens_out),
            "cost_usd": job.as_ref().map(|j| j.cost_usd),
        });
        match result {
            Ok(summary) if !summary.trim().is_empty() => {
                IssueHistory::finish_analysis(
                    pool,
                    issue_id,
                    job_id,
                    Ok((summary.trim(), &analyzed_hash)),
                )
                .await?;
                let actor = job_id
                    .map(|j| format!("agent:{j}"))
                    .unwrap_or_else(|| "agent".into());
                IssueEvent::create(
                    pool,
                    issue_id,
                    IssueEventType::HistoryAnalysis,
                    &actor,
                    &usage,
                    None,
                )
                .await?;
            }
            Ok(_) | Err(_) => {
                let error = match result {
                    Err(e) => e,
                    Ok(_) => "The history agent gave no summary".to_string(),
                };
                IssueHistory::finish_analysis(pool, issue_id, job_id, Err(&error)).await?;
                let mut payload = usage;
                payload["error"] = json!(error);
                IssueEvent::create(
                    pool,
                    issue_id,
                    IssueEventType::HistoryAnalysis,
                    "system",
                    &payload,
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lists_commits() {
        let task = history_task(
            "PROJ-1",
            "Login",
            &[("repo=\"app\" sha=\"abc\"".into(), "diff --git a b\n".into())],
        );
        assert!(task.contains("ticket PROJ-1 (\"Login\")"));
        assert!(task.contains("<commit repo=\"app\" sha=\"abc\">\ndiff --git a b\n</commit>"));
        assert!(task.contains("## What was left open"));
    }

    #[test]
    fn prior_work_hash_ignores_order() {
        assert_eq!(hash_shas(&mut ["b", "a"]), hash_shas(&mut ["a", "b"]));
        assert_ne!(hash_shas(&mut ["a"]), hash_shas(&mut ["a", "b"]));
    }
}
