//! Code transfer between the host worktree and a container (D36).
//!
//! The container never sees the host filesystem. A branch goes in as a
//! `git bundle` cloned to `/workspace/<repo>`, and comes back as a bundle the
//! host fetches into its worktree, which stays the review copy.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::{
    AGENT_USER, ExecOptions, FileEntry, Result, Sandbox, SandboxError, WORKSPACE_DIR, exec_ok,
};

const BUNDLE_IN_DIR: &str = "/tmp/kanbain-in";
const BUNDLE_OUT_DIR: &str = "/tmp/kanbain-out";

fn agent() -> ExecOptions {
    ExecOptions {
        user: Some(AGENT_USER.into()),
        ..Default::default()
    }
}

pub fn container_repo_dir(repo_name: &str) -> String {
    format!("{WORKSPACE_DIR}/{repo_name}")
}

/// Send `branch` from the host worktree into `/workspace/<repo_name>`,
/// cloning on first use and moving the branch to the host's head afterwards
/// (`checkout -B` keeps uncommitted edits that don't conflict).
/// Returns the head commit now checked out in the container.
pub async fn push_branch(
    sandbox: &dyn Sandbox,
    container: &str,
    host_worktree: &Path,
    repo_name: &str,
    branch: &str,
) -> Result<String> {
    let bundle = temp_path(repo_name, "in");
    host_git(
        host_worktree,
        &["bundle", "create", path_str(&bundle)?, branch],
    )
    .await?;
    let contents = tokio::fs::read(&bundle).await;
    let _ = tokio::fs::remove_file(&bundle).await;
    let bundle_name = format!("{repo_name}.bundle");
    sandbox
        .copy_in(
            container,
            BUNDLE_IN_DIR,
            vec![FileEntry::new(&bundle_name, contents?)],
        )
        .await?;

    let bundle_path = format!("{BUNDLE_IN_DIR}/{bundle_name}");
    let dir = container_repo_dir(repo_name);
    let exists = sandbox
        .exec(container, &["test", "-d", &format!("{dir}/.git")], &agent())
        .await?
        .exit_code
        == 0;
    if exists {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/kanbain/{branch}");
        exec_ok(
            sandbox,
            container,
            &["git", "-C", &dir, "fetch", "-q", &bundle_path, &refspec],
            &agent(),
        )
        .await?;
        exec_ok(
            sandbox,
            container,
            &[
                "git",
                "-C",
                &dir,
                "checkout",
                "-q",
                "-B",
                branch,
                &format!("refs/remotes/kanbain/{branch}"),
            ],
            &agent(),
        )
        .await?;
    } else {
        exec_ok(
            sandbox,
            container,
            &["git", "clone", "-q", "--branch", branch, &bundle_path, &dir],
            &agent(),
        )
        .await?;
        exec_ok(
            sandbox,
            container,
            &["git", "-C", &dir, "remote", "remove", "origin"],
            &agent(),
        )
        .await?;
    }
    let _ = sandbox
        .exec(container, &["rm", "-f", &bundle_path], &agent())
        .await;
    container_head(sandbox, container, repo_name, branch).await
}

/// Make the container's clone match the host worktree before a run.
///
/// - Container missing the repo, or behind the host: push the host branch.
/// - Container ahead (e.g. commits made in the sandbox terminal): pull them
///   to the host first, so nothing is overwritten.
///
/// Returns whether anything moved.
pub async fn sync_branch_in(
    sandbox: &dyn Sandbox,
    container: &str,
    host_worktree: &Path,
    repo_name: &str,
    branch: &str,
) -> Result<bool> {
    let host_head = host_git(host_worktree, &["rev-parse", "HEAD"]).await?;
    let Ok(container_head) = container_head(sandbox, container, repo_name, branch).await else {
        push_branch(sandbox, container, host_worktree, repo_name, branch).await?;
        return Ok(true);
    };
    if container_head == host_head {
        return Ok(false);
    }
    let container_behind = host_git(
        host_worktree,
        &["merge-base", "--is-ancestor", &container_head, &host_head],
    )
    .await
    .is_ok();
    if !container_behind {
        pull_branch(sandbox, container, host_worktree, repo_name, branch).await?;
        let host_head = host_git(host_worktree, &["rev-parse", "HEAD"]).await?;
        if host_head == container_head {
            return Ok(true);
        }
    }
    push_branch(sandbox, container, host_worktree, repo_name, branch).await?;
    Ok(true)
}

/// Head of `branch` in the container's clone.
pub async fn container_head(
    sandbox: &dyn Sandbox,
    container: &str,
    repo_name: &str,
    branch: &str,
) -> Result<String> {
    let dir = container_repo_dir(repo_name);
    let out = exec_ok(
        sandbox,
        container,
        &[
            "git",
            "-C",
            &dir,
            "rev-parse",
            &format!("refs/heads/{branch}"),
        ],
        &agent(),
    )
    .await?;
    Ok(out.stdout_str().trim().to_string())
}

/// Commit everything left uncommitted in the container clone. Returns
/// whether a commit was made.
pub async fn commit_all(
    sandbox: &dyn Sandbox,
    container: &str,
    repo_name: &str,
    message: &str,
) -> Result<bool> {
    let dir = container_repo_dir(repo_name);
    let status = exec_ok(
        sandbox,
        container,
        &["git", "-C", &dir, "status", "--porcelain"],
        &agent(),
    )
    .await?;
    if status.stdout_str().trim().is_empty() {
        return Ok(false);
    }
    exec_ok(
        sandbox,
        container,
        &["git", "-C", &dir, "add", "-A"],
        &agent(),
    )
    .await?;
    exec_ok(
        sandbox,
        container,
        &[
            "git",
            "-C",
            &dir,
            "commit",
            "-q",
            "--no-verify",
            "-m",
            message,
        ],
        &agent(),
    )
    .await?;
    Ok(true)
}

/// Outcome of fetching a branch back from the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    /// The container has nothing new.
    UpToDate,
    /// The host worktree moved to `head`.
    Updated { head: String },
}

/// Bring `branch` back from the container into the host worktree. The host
/// fast-forwards when it can; if the agent rewrote history the worktree is
/// moved with `reset --keep`, which refuses to drop local edits.
pub async fn pull_branch(
    sandbox: &dyn Sandbox,
    container: &str,
    host_worktree: &Path,
    repo_name: &str,
    branch: &str,
) -> Result<PullOutcome> {
    let host_head = host_git(host_worktree, &["rev-parse", "HEAD"]).await?;
    let container_head = container_head(sandbox, container, repo_name, branch).await?;
    if container_head == host_head {
        return Ok(PullOutcome::UpToDate);
    }

    let dir = container_repo_dir(repo_name);
    // Only send what the host lacks when the host head is an ancestor.
    let incremental = sandbox
        .exec(
            container,
            &[
                "git",
                "-C",
                &dir,
                "merge-base",
                "--is-ancestor",
                &host_head,
                &container_head,
            ],
            &agent(),
        )
        .await?
        .exit_code
        == 0;
    let bundle_path = format!("{BUNDLE_OUT_DIR}/{repo_name}.bundle");
    exec_ok(
        sandbox,
        container,
        &["mkdir", "-p", BUNDLE_OUT_DIR],
        &agent(),
    )
    .await?;
    let exclude = format!("^{host_head}");
    let mut cmd = vec![
        "git",
        "-C",
        &dir,
        "bundle",
        "create",
        "-q",
        &bundle_path,
        branch,
    ];
    if incremental {
        cmd.push(&exclude);
    }
    exec_ok(sandbox, container, &cmd, &agent()).await?;

    let files = sandbox.copy_out(container, &bundle_path).await?;
    let _ = sandbox
        .exec(container, &["rm", "-f", &bundle_path], &agent())
        .await;
    let (_, contents) = files
        .into_iter()
        .next()
        .ok_or_else(|| SandboxError::Other("bundle missing from container".into()))?;

    let local = temp_path(repo_name, "out");
    tokio::fs::write(&local, contents).await?;
    let result = async {
        host_git(
            host_worktree,
            &[
                "fetch",
                "-q",
                path_str(&local)?,
                &format!("refs/heads/{branch}"),
            ],
        )
        .await?;
        if host_git(host_worktree, &["merge", "-q", "--ff-only", "FETCH_HEAD"])
            .await
            .is_err()
        {
            host_git(host_worktree, &["reset", "-q", "--keep", "FETCH_HEAD"]).await?;
        }
        host_git(host_worktree, &["rev-parse", "HEAD"]).await
    }
    .await;
    let _ = tokio::fs::remove_file(&local).await;
    Ok(PullOutcome::Updated { head: result? })
}

fn temp_path(repo_name: &str, direction: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "kanbain-{direction}-{repo_name}-{}-{nanos}.bundle",
        std::process::id()
    ))
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| SandboxError::Other(format!("non-UTF-8 path {}", path.display())))
}

/// Run git on the host and return trimmed stdout.
async fn host_git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(SandboxError::Other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
