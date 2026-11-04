//! Agent-driven rebase of a workspace branch onto its target.
//!
//! The host never hands a half-done rebase to the sandbox: the conflicts of
//! a stopped rebase live in the host's index and worktree, and a git bundle
//! only carries commits. So the host's attempt is aborted, the committed
//! branch and the target go into the sandbox, and the resolver runs the
//! rebase itself, with the tickets behind both sides spelled out so it
//! merges intent instead of guessing at hunks.

use std::{collections::BTreeMap, path::Path};

use db::models::{
    conflict_resolution::{ConflictResolution, ConflictResolutionStage, CreateConflictResolution},
    issue::Issue,
    project::Project,
    project_status::ProjectStatus,
    repo::Repo,
    workspace::Workspace,
};
use git::{GitCli, GitCliError};
use sqlx::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

use crate::sandbox::RepoSync;

/// How much of a ticket description goes into the context file.
const DESCRIPTION_CHARS: usize = 2000;
/// Target commits listed at most; a branch 500 commits behind is a
/// conversation, not a context file.
const MAX_COMMITS: usize = 40;

#[derive(Debug, Error)]
pub enum ConflictResolutionError {
    #[error(
        "The branch has uncommitted changes. Commit or discard them first: the rebase runs on \
         the committed branch."
    )]
    Dirty,
    #[error("The branch already contains {0}; there is nothing to rebase.")]
    UpToDate(String),
    #[error(transparent)]
    Git(#[from] GitCliError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A handed-off resolution and the prompt for its run.
pub struct Begun {
    pub resolution: ConflictResolution,
    pub prompt: String,
}

/// Abort whatever the host worktree is stuck in, check the branch is
/// committed and actually behind, predict the conflicts and record the
/// attempt. The resolver run picks the row up when it starts.
pub async fn begin(
    pool: &SqlitePool,
    workspace: &Workspace,
    repo: &Repo,
    target_branch: &str,
    worktree: &Path,
) -> Result<Begun, ConflictResolutionError> {
    let git = GitCli::new();

    // The host's stopped rebase is dead weight now; the sandbox redoes it.
    git.abort_rebase(worktree)?;
    git.abort_merge(worktree)?;
    git.abort_cherry_pick(worktree)?;
    git.abort_revert(worktree)?;

    let dirty = git.git(worktree, ["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        return Err(ConflictResolutionError::Dirty);
    }

    let target_oid = rev_parse(&git, worktree, &format!("{target_branch}^{{commit}}"))?;
    let branch_oid = rev_parse(&git, worktree, "HEAD")?;
    if is_ancestor(&git, worktree, &target_oid, &branch_oid) {
        return Err(ConflictResolutionError::UpToDate(target_branch.to_string()));
    }

    let predicted = match git.predict_conflicts(worktree, &branch_oid, &target_oid) {
        Ok(Some(files)) => files,
        // Old git: files both sides touched are the best guess.
        _ => files_touched_by_both(&git, worktree, &branch_oid, &target_oid),
    };

    let resolution = ConflictResolution::create(
        pool,
        CreateConflictResolution {
            workspace_id: workspace.id,
            repo_id: repo.id,
            target_branch,
            target_oid: &target_oid,
            branch_oid: &branch_oid,
            predicted_files: &predicted,
        },
    )
    .await?;
    let prompt = prompt(&workspace.branch, target_branch, &repo.name, &predicted);
    Ok(Begun { resolution, prompt })
}

/// The follow-up the resolver gets. The steps are spelled out because the
/// failure mode is an agent that "finds no conflicts" and commits nothing.
pub fn prompt(branch: &str, target: &str, repo_name: &str, predicted: &[String]) -> String {
    let files = if predicted.is_empty() {
        String::new()
    } else {
        let mut list = predicted
            .iter()
            .take(20)
            .map(|f| format!("- {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        if predicted.len() > 20 {
            list.push_str(&format!("\n- ...and {} more", predicted.len() - 20));
        }
        format!("\nExpected conflicts:\n{list}\n")
    };
    format!(
        "Rebase `{branch}` onto `{target}` in repository `{repo_name}` and resolve the \
         conflicts.\n\n\
         The branch is committed and clean. No rebase is running yet: you start it.\n\
         Read /kanbain/context/rebase.md first. It lists what changed on `{target}` in the \
         conflicting files and the tickets those changes came from. Your own ticket is \
         /kanbain/context/ticket.md.\n{files}\n\
         Steps:\n\
         1. `cd /workspace/{repo_name}` and check `git status` is clean.\n\
         2. `GIT_EDITOR=true git rebase {target}`\n\
         3. At every stop, resolve each conflicted file so the change from your ticket \
         AND the change from `{target}` both survive. Never just pick a side: if they \
         contradict, follow what each ticket asked for and say how you combined them. \
         Then `git add` the files and `GIT_EDITOR=true git rebase --continue`.\n\
         4. When the rebase is done, build and run the tests that cover the conflicting \
         code. Commit any fixes on top.\n\
         5. You are done when `git status` shows no rebase in progress and \
         `git merge-base --is-ancestor {target} HEAD` succeeds.\n\n\
         Do not `git rebase --abort` to get out of a conflict, do not merge, do not push.\n\
         Finish with a short summary per conflicted file: what each side wanted and what \
         you kept."
    )
}

/// `rebase.md`: both sides of the conflict as tickets and commits.
pub async fn context_files(
    pool: &SqlitePool,
    workspace: &Workspace,
    repo: &Repo,
    resolution: &ConflictResolution,
    worktree: &Path,
) -> Vec<(String, String)> {
    let git = GitCli::new();
    let target = &resolution.target_branch;
    let files = &resolution.predicted_files.0;
    let merge_base = git
        .git(
            worktree,
            ["merge-base", &resolution.branch_oid, &resolution.target_oid],
        )
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let target_commits = if merge_base.is_empty() {
        vec![]
    } else {
        log_commits(&git, worktree, &merge_base, &resolution.target_oid, files)
    };
    let own_commits = if merge_base.is_empty() {
        vec![]
    } else {
        log_commits(&git, worktree, &merge_base, &resolution.branch_oid, files)
    };

    let (project, own_issue) = match workspace.issue_id {
        Some(id) => match Issue::find_by_id(pool, id).await.ok().flatten() {
            Some(issue) => (
                Project::find_by_id(pool, issue.project_id)
                    .await
                    .ok()
                    .flatten(),
                Some(issue),
            ),
            None => (None, None),
        },
        None => (None, None),
    };

    // Which ticket each target commit belongs to: the history index first
    // (it already matched commits to tickets), then the `ID: ` subject
    // prefix every KanbAIn commit and merge carries.
    let mut by_ticket: BTreeMap<Uuid, Vec<&Commit>> = BTreeMap::new();
    let mut unattributed: Vec<&Commit> = Vec::new();
    for commit in &target_commits {
        let mut ids = indexed_tickets(pool, repo.id, &commit.sha).await;
        if ids.is_empty()
            && let Some(project) = &project
            && let Some(id) = subject_ticket(&commit.subject)
            && let Some(issue_id) = ticket_by_display_id(pool, project, &id).await
        {
            ids.push(issue_id);
        }
        ids.retain(|id| Some(*id) != workspace.issue_id);
        if ids.is_empty() {
            unattributed.push(commit);
        }
        for id in ids {
            by_ticket.entry(id).or_default().push(commit);
        }
    }

    let prefix = project
        .as_ref()
        .map(|p| p.id_prefix.clone())
        .unwrap_or_else(|| "KAN".into());
    let own_id = own_issue
        .as_ref()
        .map(|i| i.display_id(&prefix))
        .unwrap_or_else(|| workspace.branch.clone());

    let mut md = format!(
        "# Rebasing {own_id} onto `{target}`\n\n\
         Your branch: `{}` at {}. Target: `{target}` at {}.\n\
         Merge base: {}.\n",
        workspace.branch,
        short(&resolution.branch_oid),
        short(&resolution.target_oid),
        if merge_base.is_empty() {
            "unknown".to_string()
        } else {
            short(&merge_base).to_string()
        },
    );
    if files.is_empty() {
        md.push_str(
            "\n`git merge-tree` predicted no conflicts, so the rebase may stop on \
             commit-by-commit conflicts it couldn't see. Handle them the same way.\n",
        );
    } else {
        md.push_str("\n## Expected conflicts\n\n");
        for f in files {
            md.push_str(&format!("- `{f}`\n"));
        }
        md.push_str(&format!(
            "\nSee each side with `git diff {mb} {target} -- <file>` (theirs) and \
             `git diff {mb} HEAD -- <file>` (yours).\n",
            mb = short(&merge_base),
        ));
    }

    md.push_str(&format!(
        "\n## What landed on `{target}` since you branched\n\n\
         These tickets changed the conflicting code. Their intent has to survive the \
         rebase just like yours.\n"
    ));
    if by_ticket.is_empty() && unattributed.is_empty() {
        md.push_str("\nNo commits on the target touch the expected files.\n");
    }
    for (issue_id, commits) in &by_ticket {
        let Ok(Some(issue)) = Issue::find_by_id(pool, *issue_id).await else {
            continue;
        };
        let status = ProjectStatus::find_by_id(pool, issue.status_id)
            .await
            .ok()
            .flatten()
            .map(|s| s.name)
            .unwrap_or_default();
        md.push_str(&format!(
            "\n### {}: {} ({status})\n\n",
            issue.display_id(&prefix),
            issue.title
        ));
        let description = issue.description.as_deref().unwrap_or("").trim();
        if description.is_empty() {
            md.push_str("_No description._\n");
        } else {
            md.push_str(&excerpt(description, DESCRIPTION_CHARS));
            md.push('\n');
        }
        md.push_str("\nCommits:\n");
        for c in commits {
            md.push_str(&c.line());
        }
    }
    if !unattributed.is_empty() {
        md.push_str("\n### Commits with no ticket\n\n");
        for c in &unattributed {
            md.push_str(&c.line());
        }
    }

    md.push_str(&format!(
        "\n## Your side ({own_id})\n\nThe ticket is in /kanbain/context/ticket.md. \
         Your commits on the expected files:\n\n"
    ));
    if own_commits.is_empty() {
        md.push_str("None found.\n");
    }
    for c in &own_commits {
        md.push_str(&c.line());
    }

    vec![("rebase.md".to_string(), md)]
}

/// Called once the resolver's run ended and the sandbox handed the branch
/// back: decide from the host's actual state whether it worked.
pub async fn finish(
    pool: &SqlitePool,
    resolution: &ConflictResolution,
    repo_name: &str,
    worktree: &Path,
    reports: &[RepoSync],
) {
    let outcome = verify(resolution, repo_name, worktree, reports);
    let (stage, error) = match &outcome {
        Ok(()) => (ConflictResolutionStage::Resolved, None),
        Err(reason) => (ConflictResolutionStage::Failed, Some(reason.as_str())),
    };
    if let Err(e) = ConflictResolution::set_stage(pool, resolution.id, stage, error).await {
        tracing::error!(
            "Recording conflict resolution {} failed: {e}",
            resolution.id
        );
    }
}

fn verify(
    resolution: &ConflictResolution,
    repo_name: &str,
    worktree: &Path,
    reports: &[RepoSync],
) -> Result<(), String> {
    let target = &resolution.target_branch;
    if let Some(report) = reports.iter().find(|r| r.repo == repo_name) {
        if let Some(op) = &report.aborted_op {
            return Err(format!(
                "The agent stopped in the middle of the {op}. It was rolled back; the \
                 branch is unchanged."
            ));
        }
        if let Some(error) = &report.error {
            return Err(format!("Bringing the branch back failed: {error}"));
        }
    }
    let git = GitCli::new();
    if git.is_rebase_in_progress(worktree).unwrap_or(false)
        || git.is_merge_in_progress(worktree).unwrap_or(false)
    {
        return Err("The host worktree is in the middle of a rebase or merge.".to_string());
    }
    let conflicted = git.get_conflicted_files(worktree).unwrap_or_default();
    if !conflicted.is_empty() {
        return Err(format!(
            "The host worktree still has conflicts in {}",
            conflicted.join(", ")
        ));
    }
    let head = rev_parse(&git, worktree, "HEAD").map_err(|e| e.to_string())?;
    if !is_ancestor(&git, worktree, &resolution.target_oid, &head) {
        return Err(format!(
            "The branch is still not on top of `{target}`: the rebase never finished."
        ));
    }
    // Markers committed as "resolved" are the sneakiest way to fail.
    let files = &resolution.predicted_files.0;
    if !files.is_empty() {
        let mut args = vec![
            "grep".to_string(),
            "-l".into(),
            "-E".into(),
            "^(<<<<<<<|>>>>>>>)( |$)".into(),
            "HEAD".into(),
            "--".into(),
        ];
        args.extend(files.iter().cloned());
        if let Ok(out) = git.git(worktree, &args) {
            let hits: Vec<&str> = out
                .lines()
                .map(|l| l.trim_start_matches("HEAD:"))
                .filter(|l| !l.is_empty())
                .collect();
            if !hits.is_empty() {
                return Err(format!(
                    "Conflict markers were committed in {}",
                    hits.join(", ")
                ));
            }
        }
    }
    Ok(())
}

struct Commit {
    sha: String,
    subject: String,
}

impl Commit {
    fn line(&self) -> String {
        format!("- {} {}\n", short(&self.sha), self.subject)
    }
}

/// Commits in `base..head` touching `files` (all files when empty), newest
/// first.
fn log_commits(git: &GitCli, dir: &Path, base: &str, head: &str, files: &[String]) -> Vec<Commit> {
    let mut args = vec![
        "log".to_string(),
        "--no-merges".into(),
        format!("--max-count={MAX_COMMITS}"),
        "--format=%H%x1f%s".into(),
        format!("{base}..{head}"),
    ];
    if !files.is_empty() {
        args.push("--".into());
        args.extend(files.iter().cloned());
    }
    git.git(dir, &args)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (sha, subject) = line.split_once('\x1f')?;
            Some(Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

fn files_touched_by_both(git: &GitCli, dir: &Path, ours: &str, theirs: &str) -> Vec<String> {
    let Ok(base) = git.git(dir, ["merge-base", ours, theirs]) else {
        return vec![];
    };
    let base = base.trim();
    let changed = |head: &str| -> Vec<String> {
        git.git(dir, ["diff", "--name-only", base, head])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    };
    let theirs = changed(theirs);
    changed(ours)
        .into_iter()
        .filter(|f| theirs.contains(f))
        .collect()
}

fn rev_parse(git: &GitCli, dir: &Path, rev: &str) -> Result<String, GitCliError> {
    Ok(git
        .git(dir, ["rev-parse", "--verify", "-q", rev])?
        .trim()
        .to_string())
}

fn is_ancestor(git: &GitCli, dir: &Path, ancestor: &str, head: &str) -> bool {
    git.git(dir, ["merge-base", "--is-ancestor", ancestor, head])
        .is_ok()
}

/// Full ref name of `branch` (`refs/heads/main`, `refs/remotes/origin/main`).
pub fn full_ref(dir: &Path, branch: &str) -> Option<String> {
    GitCli::new()
        .git(dir, ["rev-parse", "--symbolic-full-name", branch])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("refs/"))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}...")
}

/// `PAL-51` from `PAL-51: cat walks forward`.
fn subject_ticket(subject: &str) -> Option<String> {
    let (id, _) = subject.split_once(':')?;
    let id = id.trim();
    let (prefix, number) = id.rsplit_once('-')?;
    let prefix_ok = !prefix.is_empty()
        && prefix.starts_with(|c: char| c.is_ascii_alphabetic())
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let number_ok = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    (prefix_ok && number_ok).then(|| id.to_string())
}

async fn indexed_tickets(pool: &SqlitePool, repo_id: Uuid, sha: &str) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT issue_id FROM ticket_commits WHERE repo_id = $1 AND sha = $2",
    )
    .bind(repo_id)
    .bind(sha)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// The project's ticket a display id names: its external id, or
/// `<prefix>-<number>`.
async fn ticket_by_display_id(pool: &SqlitePool, project: &Project, id: &str) -> Option<Uuid> {
    let number = id
        .strip_prefix(&format!("{}-", project.id_prefix))
        .and_then(|n| n.parse::<i64>().ok());
    sqlx::query_scalar::<_, Uuid>(
        r#"SELECT id FROM issues
           WHERE project_id = $1 AND (external_id = $2 OR ($3 IS NOT NULL AND number = $3))
           LIMIT 1"#,
    )
    .bind(project.id)
    .bind(id)
    .bind(number)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_ticket_takes_only_id_prefixes() {
        assert_eq!(
            subject_ticket("PAL-51: cat walks forward").as_deref(),
            Some("PAL-51")
        );
        assert_eq!(
            subject_ticket("JIRA_X-7: thing").as_deref(),
            Some("JIRA_X-7")
        );
        assert_eq!(subject_ticket("fix: typo"), None);
        assert_eq!(subject_ticket("Merge branch 'main'"), None);
        assert_eq!(subject_ticket("PAL-: nope"), None);
        assert_eq!(subject_ticket("51-PAL: nope"), None);
    }

    #[test]
    fn prompt_names_both_branches_and_files() {
        let p = prompt(
            "kanbain/PAL-51-cat",
            "main",
            "desktop-pal",
            &["src/app.rs".to_string()],
        );
        assert!(p.contains("GIT_EDITOR=true git rebase main"));
        assert!(p.contains("- src/app.rs"));
        assert!(p.contains("/workspace/desktop-pal"));
        assert!(p.contains("/kanbain/context/rebase.md"));
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, body: &str, msg: &str) {
        std::fs::write(dir.join(file), body).unwrap();
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", msg]);
    }

    /// main and feature both rewrite `app.rs`; `other.rs` only on main.
    fn diverged() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        sh(&dir, &["init", "-q", "-b", "main"]);
        commit(&dir, "app.rs", "fn a() {}\n", "init");
        sh(&dir, &["switch", "-q", "-c", "feature"]);
        commit(&dir, "app.rs", "fn a() { feature(); }\n", "PAL-51: feature");
        sh(&dir, &["switch", "-q", "main"]);
        commit(&dir, "app.rs", "fn a() { main(); }\n", "PAL-42: main side");
        commit(&dir, "other.rs", "x\n", "PAL-43: unrelated");
        sh(&dir, &["switch", "-q", "feature"]);
        (tmp, dir)
    }

    fn resolution(dir: &Path, files: Vec<String>) -> ConflictResolution {
        ConflictResolution {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            repo_id: Uuid::new_v4(),
            execution_process_id: None,
            target_branch: "main".into(),
            target_oid: sh(dir, &["rev-parse", "main"]),
            branch_oid: sh(dir, &["rev-parse", "feature"]),
            predicted_files: sqlx::types::Json(files),
            stage: ConflictResolutionStage::Syncing,
            error: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn predicts_the_real_conflict_and_lists_both_sides() {
        let (_tmp, dir) = diverged();
        let git = GitCli::new();
        let main = sh(&dir, &["rev-parse", "main"]);
        let feature = sh(&dir, &["rev-parse", "feature"]);
        if let Ok(Some(files)) = git.predict_conflicts(&dir, &feature, &main) {
            assert_eq!(files, ["app.rs"]);
        }
        assert_eq!(
            files_touched_by_both(&git, &dir, &feature, &main),
            ["app.rs"]
        );
        let base = sh(&dir, &["merge-base", "main", "feature"]);
        let theirs = log_commits(&git, &dir, &base, &main, &["app.rs".into()]);
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].subject, "PAL-42: main side");
        assert_eq!(
            subject_ticket(&theirs[0].subject).as_deref(),
            Some("PAL-42")
        );
    }

    #[test]
    fn verify_fails_until_the_branch_sits_on_the_target() {
        let (_tmp, dir) = diverged();
        let r = resolution(&dir, vec!["app.rs".into()]);
        let err = verify(&r, "app", &dir, &[]).unwrap_err();
        assert!(err.contains("not on top of `main`"), "{err}");

        // Stopped mid-rebase is not done, even with the conflict staged.
        sh(&dir, &["rebase", "-q", "main"]);
        assert!(GitCli::new().is_rebase_in_progress(&dir).unwrap());
        sh(&dir, &["add", "-A"]);
        let err = verify(&r, "app", &dir, &[]).unwrap_err();
        assert!(err.contains("middle of a rebase"), "{err}");

        // A "resolution" that commits the markers is not one either.
        sh(&dir, &["-c", "core.editor=true", "rebase", "--continue"]);
        assert!(!GitCli::new().is_rebase_in_progress(&dir).unwrap());
        assert!(
            std::fs::read_to_string(dir.join("app.rs"))
                .unwrap()
                .contains("<<<<<<<")
        );
        let err = verify(&r, "app", &dir, &[]).unwrap_err();
        assert!(err.contains("Conflict markers"), "{err}");

        commit(
            &dir,
            "app.rs",
            "fn a() { main(); feature(); }\n",
            "PAL-51: merge both",
        );
        assert_eq!(verify(&r, "app", &dir, &[]), Ok(()));
    }

    #[test]
    fn verify_reports_what_the_sandbox_said() {
        let (_tmp, dir) = diverged();
        let r = resolution(&dir, vec![]);
        let aborted = RepoSync {
            repo: "app".into(),
            aborted_op: Some("rebase".into()),
            error: None,
        };
        let err = verify(&r, "app", &dir, &[aborted]).unwrap_err();
        assert!(err.contains("middle of the rebase"), "{err}");
        let failed = RepoSync {
            repo: "app".into(),
            aborted_op: None,
            error: Some("boom".into()),
        };
        let err = verify(&r, "app", &dir, &[failed]).unwrap_err();
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn excerpt_cuts_on_chars() {
        assert_eq!(excerpt("äöü", 5), "äöü");
        assert_eq!(excerpt("äöüß", 2), "äö...");
    }
}
