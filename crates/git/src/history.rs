//! The git-history check (D18): commits that mention a ticket id in their
//! message, or sit on a branch named after it, with the files they changed
//! (E19 ticket ↔ code links).

use std::{collections::BTreeMap, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{GitCli, GitCliError};

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitFile {
    pub path: String,
    /// `None` for binary files.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryCommit {
    pub sha: String,
    pub author: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
    pub body: String,
    /// Branches named after the id that contain the commit.
    pub branches: Vec<String>,
    /// The message mentions the id.
    pub message_match: bool,
    pub files: Vec<CommitFile>,
}

impl HistoryCommit {
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(10)]
    }
}

/// Whether `text` contains `id` as a whole token, case-insensitively:
/// `PROJ-12` matches `[PROJ-12]` and `proj-12: fix`, not `PROJ-123`.
pub fn mentions_id(text: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let hay = text.to_ascii_lowercase();
    let needle = id.to_ascii_lowercase();
    let mut start = 0;
    while let Some(pos) = hay[start..].find(&needle) {
        let at = start + pos;
        let end = at + needle.len();
        let before_ok = hay[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let after_ok = hay[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Whether the commit message starts with the id (E19: KanbAIn's own
/// commits are `<ID>: …`).
pub fn message_starts_with_id(message: &str, id: &str) -> bool {
    let message = message.trim_start().trim_start_matches('[');
    message.len() >= id.len()
        && message[..id.len()].eq_ignore_ascii_case(id)
        && message[id.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric())
}

fn parse_log(out: &str) -> Vec<(String, String, DateTime<Utc>, String, String)> {
    out.split(RECORD)
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut fields = record.splitn(5, FIELD);
            let sha = fields.next()?.trim().to_string();
            if sha.is_empty() {
                return None;
            }
            let author = fields.next()?.to_string();
            let date = DateTime::parse_from_rfc3339(fields.next()?.trim())
                .ok()?
                .with_timezone(&Utc);
            let subject = fields.next()?.to_string();
            let body = fields.next().unwrap_or("").trim().to_string();
            Some((sha, author, date, subject, body))
        })
        .collect()
}

const LOG_FORMAT: &str = "--format=%H%x1f%an%x1f%cI%x1f%s%x1f%b%x1e";

fn numstat(git: &GitCli, repo: &Path, sha: &str) -> Result<Vec<CommitFile>, GitCliError> {
    let out = git.git(
        repo,
        ["show", "--numstat", "--format=", "--no-renames", sha],
    )?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let additions = parts.next()?.parse().ok();
            let deletions = parts.next()?.parse().ok();
            let path = parts.next()?.trim().to_string();
            (!path.is_empty()).then_some(CommitFile {
                path,
                additions,
                deletions,
            })
        })
        .collect())
}

/// Commits in `repo` for ticket `id`: messages mentioning it on any ref,
/// plus commits on branches named after it that the checked-out branch
/// doesn't contain. Newest first, at most `limit`.
pub fn find_ticket_commits(
    repo: &Path,
    id: &str,
    limit: usize,
) -> Result<Vec<HistoryCommit>, GitCliError> {
    let git = GitCli::new();
    let limit_arg = format!("--max-count={limit}");
    let mut found: BTreeMap<String, HistoryCommit> = BTreeMap::new();

    let grep = format!("--grep={id}");
    let out = git.git(
        repo,
        [
            "log",
            "--all",
            "--regexp-ignore-case",
            "--fixed-strings",
            grep.as_str(),
            LOG_FORMAT,
            limit_arg.as_str(),
        ],
    )?;
    for (sha, author, committed_at, subject, body) in parse_log(&out) {
        if !mentions_id(&format!("{subject}\n{body}"), id) {
            continue;
        }
        found.insert(
            sha.clone(),
            HistoryCommit {
                sha,
                author,
                committed_at,
                subject,
                body,
                branches: vec![],
                message_match: true,
                files: vec![],
            },
        );
    }

    let refs = git.git(
        repo,
        [
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let has_head = git
        .git(repo, ["rev-parse", "--verify", "-q", "HEAD"])
        .is_ok();
    for full in refs.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let short = full
            .strip_prefix("refs/heads/")
            .or_else(|| full.strip_prefix("refs/remotes/"))
            .unwrap_or(full);
        if short.ends_with("/HEAD") || !mentions_id(short, id) {
            continue;
        }
        let mut args = vec!["log", full];
        if has_head {
            args.extend(["--not", "HEAD"]);
        }
        args.extend([LOG_FORMAT, limit_arg.as_str()]);
        let out = git.git(repo, args)?;
        for (sha, author, committed_at, subject, body) in parse_log(&out) {
            let entry = found.entry(sha.clone()).or_insert_with(|| HistoryCommit {
                sha,
                author,
                committed_at,
                subject,
                body,
                branches: vec![],
                message_match: false,
                files: vec![],
            });
            if !entry.branches.iter().any(|b| b == short) {
                entry.branches.push(short.to_string());
            }
        }
    }

    let mut commits: Vec<HistoryCommit> = found.into_values().collect();
    commits.sort_by(|a, b| b.committed_at.cmp(&a.committed_at));
    commits.truncate(limit);
    for commit in &mut commits {
        commit.files = numstat(&git, repo, &commit.sha)?;
    }
    Ok(commits)
}

/// A commit's message and patch for the history agent. A patch longer than
/// `max_bytes` is reduced to the list of changed files; the flag says so.
pub fn commit_patch(
    repo: &Path,
    sha: &str,
    max_bytes: usize,
) -> Result<(String, bool), GitCliError> {
    let git = GitCli::new();
    let full = git.git(
        repo,
        [
            "show",
            "--format=fuller",
            "--stat",
            "--patch",
            "--no-color",
            sha,
        ],
    )?;
    if full.len() <= max_bytes {
        return Ok((full, false));
    }
    let stat = git.git(
        repo,
        ["show", "--format=fuller", "--stat", "--no-color", sha],
    )?;
    Ok((stat, true))
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use super::*;

    fn run(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit(repo: &Path, file: &str, contents: &str, message: &str) {
        fs::write(repo.join(file), contents).unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-q", "-m", message]);
    }

    #[test]
    fn id_matching() {
        assert!(mentions_id("Fix [PROJ-12] login", "PROJ-12"));
        assert!(mentions_id("proj-12: fix", "PROJ-12"));
        assert!(!mentions_id("PROJ-123 other", "PROJ-12"));
        assert!(!mentions_id("XPROJ-12", "PROJ-12"));
        assert!(mentions_id("feature/proj-12-login", "PROJ-12"));
        assert!(message_starts_with_id("PROJ-12: fix", "PROJ-12"));
        assert!(message_starts_with_id("[proj-12] fix", "PROJ-12"));
        assert!(!message_starts_with_id("Fix PROJ-12", "PROJ-12"));
        assert!(!message_starts_with_id("PROJ-123: x", "PROJ-12"));
    }

    #[test]
    fn finds_messages_and_branches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        commit(repo, "a.txt", "1\n", "Initial");
        commit(repo, "login.rs", "fn login() {}\n", "PROJ-12: Add login");
        commit(repo, "other.rs", "x\n", "PROJ-123: unrelated");
        run(repo, &["checkout", "-q", "-b", "feature/proj-12-reset"]);
        commit(
            repo,
            "reset.rs",
            "fn reset() {}\nfn more() {}\n",
            "Reset flow",
        );
        run(repo, &["checkout", "-q", "main"]);
        commit(
            repo,
            "a.txt",
            "2\n",
            "Follow-up for proj-12\n\nDetails here",
        );

        let commits = find_ticket_commits(repo, "PROJ-12", 50).unwrap();
        let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects.len(), 3, "{subjects:?}");
        assert!(subjects.contains(&"PROJ-12: Add login"));
        assert!(subjects.contains(&"Reset flow"));
        assert!(subjects.contains(&"Follow-up for proj-12"));

        let reset = commits.iter().find(|c| c.subject == "Reset flow").unwrap();
        assert!(!reset.message_match);
        assert_eq!(reset.branches, vec!["feature/proj-12-reset"]);
        assert_eq!(
            reset.files,
            vec![CommitFile {
                path: "reset.rs".into(),
                additions: Some(2),
                deletions: Some(0),
            }]
        );
        let follow = commits
            .iter()
            .find(|c| c.subject == "Follow-up for proj-12")
            .unwrap();
        assert_eq!(follow.body, "Details here");

        let (patch, reduced) = commit_patch(repo, &reset.sha, 100_000).unwrap();
        assert!(!reduced);
        assert!(patch.contains("+fn reset() {}"));
        let (stat, reduced) = commit_patch(repo, &reset.sha, 10).unwrap();
        assert!(reduced);
        assert!(stat.contains("reset.rs") && !stat.contains("+fn reset"));

        assert!(find_ticket_commits(repo, "NOPE-1", 50).unwrap().is_empty());
    }
}
