//! The scribe (Phase 7, D25): after a coder, tester or fix run is merged
//! into the ticket branch, a headless Haiku job reads that merge's diff
//! and writes the changelog entry. Files and commits come from git, not
//! from the model; the model only writes the prose. When it fails, the
//! entry still lands with a plain summary built from the commits, because
//! a changelog with holes in it is worse than a boring one.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use db::models::{
    changelog_entry::{ChangelogEntry, ChangelogKind, CommitRef, CreateChangelogEntry},
    issue::Issue,
    issue_event::IssueEventType,
    issue_found::IssueFound,
    project::Project,
};
use executors::adapters::AgentRole;
use git::GitCli;
use serde_json::json;
use uuid::Uuid;

use super::OrchestratorService;
use crate::jobs::JobRequest;

pub(super) const JOB_KIND_SCRIBE: &str = "scribe";
/// Patch text the scribe gets in total; past this, files are listed only.
const MAX_DIFF_BYTES: usize = 60_000;
/// Patch text per file before it is cut.
const MAX_FILE_DIFF_BYTES: usize = 12_000;

/// One run to write up.
pub(super) struct ScribeInput {
    pub issue_id: Uuid,
    pub run_id: Option<Uuid>,
    pub kind: ChangelogKind,
    /// e.g. "Subtask 2: Add the login form"
    pub title: String,
    /// What the run was for: the subtask description or the checkpoint.
    pub what: String,
    /// Merge commit per repo on the ticket branch.
    pub commits: BTreeMap<String, String>,
    /// The ticket workspace root, holding one worktree per repo.
    pub ticket_root: PathBuf,
    pub issues_fixed: Vec<IssueFound>,
    pub issues_found: Vec<IssueFound>,
}

/// What one merge commit changed.
struct RepoChange {
    repo: String,
    sha: String,
    subjects: Vec<String>,
    files: Vec<String>,
    patch: String,
}

impl OrchestratorService {
    /// Writes the entry in the background: the orchestrator holds the
    /// ticket lock and must not wait for a model.
    pub(super) fn spawn_scribe(&self, input: ScribeInput) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.scribe(input).await {
                tracing::warn!("Writing a changelog entry failed: {e}");
            }
        });
    }

    async fn scribe(&self, input: ScribeInput) -> Result<(), sqlx::Error> {
        let pool = &self.db.pool;
        if let Some(run) = input.run_id
            && ChangelogEntry::exists_for_run(pool, run, input.kind).await?
        {
            return Ok(());
        }
        let Some(issue) = Issue::find_by_id(pool, input.issue_id).await? else {
            return Ok(());
        };
        let project = Project::find_by_id(pool, issue.project_id).await?;
        let id = self.display_id(&issue).await;

        let root = input.ticket_root.clone();
        let commits = input.commits.clone();
        let changes = tokio::task::spawn_blocking(move || collect_changes(&root, &commits))
            .await
            .unwrap_or_default();
        let files: Vec<String> = changes
            .iter()
            .flat_map(|c| c.files.iter().map(move |f| format!("{}/{f}", c.repo)))
            .collect();
        let commit_refs: Vec<CommitRef> = changes
            .iter()
            .map(|c| CommitRef {
                repo: c.repo.clone(),
                sha: c.sha.clone(),
            })
            .collect();

        let task = scribe_task(&id, &issue.title, &input, &changes);
        let (job_id, result) = self
            .jobs
            .run(JobRequest {
                kind: JOB_KIND_SCRIBE,
                role: AgentRole::Scribe,
                project,
                issue_id: Some(issue.id),
                context: vec![("run.md".to_string(), task.clone())],
                task,
                repos: Vec::new(),
            })
            .await;
        let written = result.map(|out| {
            out.files
                .get("changelog.md")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(out.message)
        });
        let (summary, scribe_error) = match written {
            Ok(text) if !text.trim().is_empty() => (text.trim().to_string(), None),
            Ok(_) => (
                fallback_summary(&input, &changes),
                Some("The scribe wrote nothing".to_string()),
            ),
            Err(e) => (fallback_summary(&input, &changes), Some(e)),
        };

        let fixed: Vec<Uuid> = input.issues_fixed.iter().map(|i| i.id).collect();
        let found: Vec<Uuid> = input.issues_found.iter().map(|i| i.id).collect();
        let Some(entry) = ChangelogEntry::create(
            pool,
            CreateChangelogEntry {
                issue_id: issue.id,
                run_id: input.run_id,
                job_id,
                kind: input.kind,
                title: &input.title,
                summary: &summary,
                files: &files,
                commit_refs: &commit_refs,
                issues_fixed: &fixed,
                issues_found: &found,
                scribe_error: scribe_error.as_deref(),
            },
        )
        .await?
        else {
            return Ok(());
        };
        // The event also re-indexes the ticket, so the entry is searchable.
        let actor = job_id
            .map(|j| format!("agent:{j}"))
            .unwrap_or_else(|| "system".into());
        self.event(
            issue.id,
            IssueEventType::ChangelogEntry,
            &actor,
            json!({"entry": entry.id, "kind": entry.kind, "title": entry.title,
                   "files": files.len(), "error": scribe_error}),
        )
        .await;
        Ok(())
    }
}

fn collect_changes(root: &Path, commits: &BTreeMap<String, String>) -> Vec<RepoChange> {
    let git = GitCli::new();
    let mut budget = MAX_DIFF_BYTES;
    let mut out = Vec::new();
    for (repo, sha) in commits {
        let dir = root.join(repo);
        // `sha^1..sha` covers a squash and a merge commit alike: what the
        // ticket branch gained.
        let range = format!("{sha}^1..{sha}");
        let files: Vec<String> = git
            .git(&dir, ["diff", "--name-only", &range])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect();
        let subjects: Vec<String> = git
            .git(&dir, ["log", "--format=%s", &range])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let mut patch = String::new();
        for file in &files {
            if budget == 0 {
                break;
            }
            let text = git
                .git(&dir, ["diff", &range, "--", file])
                .unwrap_or_default();
            let text = cut(&text, MAX_FILE_DIFF_BYTES.min(budget));
            budget = budget.saturating_sub(text.len());
            patch.push_str(text);
            if !text.ends_with('\n') {
                patch.push('\n');
            }
        }
        out.push(RepoChange {
            repo: repo.clone(),
            sha: sha.clone(),
            subjects,
            files,
            patch,
        });
    }
    out
}

fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn issue_line(i: &IssueFound) -> String {
    let at = match (&i.file, i.line) {
        (Some(f), Some(l)) => format!(" ({f}:{l})"),
        (Some(f), None) => format!(" ({f})"),
        _ => String::new(),
    };
    format!("- [{}] {}{at}", i.severity, i.description)
}

/// The scribe's first message, with everything inline: it has no repo.
fn scribe_task(
    id: &str,
    ticket_title: &str,
    input: &ScribeInput,
    changes: &[RepoChange],
) -> String {
    let kind = match input.kind {
        ChangelogKind::Coder => "a coder run",
        ChangelogKind::Tester => "a tester run",
        ChangelogKind::Fix => "a fix run",
        ChangelogKind::Merge => "a merge",
    };
    let mut task = format!(
        "Write the changelog entry for {kind} on ticket {id} (\"{ticket_title}\"): \
         {}.\n\n\
         Answer with Markdown only, in these sections, and also write the same text to \
         /kanbain/out/changelog.md if you can:\n\
         ## What changed\n## Why\n## Files\n## Issues found and fixed\n\n\
         Be concrete and short: name files, functions and behaviour. Use only what the \
         diff and the lists below show; say \"none\" for an empty section. No preamble.\n\n\
         ## What the run was for\n\n{}\n",
        input.title,
        input.what.trim()
    );
    if !input.issues_found.is_empty() {
        task.push_str("\n## Issues the tester found\n\n");
        for i in &input.issues_found {
            task.push_str(&issue_line(i));
            task.push('\n');
        }
    }
    if !input.issues_fixed.is_empty() {
        let label = if input.kind == ChangelogKind::Fix {
            "Issues this run was sent to fix"
        } else {
            "Issues the re-test confirmed fixed"
        };
        task.push_str(&format!("\n## {label}\n\n"));
        for i in &input.issues_fixed {
            task.push_str(&issue_line(i));
            task.push('\n');
        }
    }
    for c in changes {
        task.push_str(&format!(
            "\n<repo name=\"{}\" commit=\"{}\">\nCommits:\n{}\n\nFiles:\n{}\n\n```diff\n{}```\n</repo>\n",
            c.repo,
            &c.sha[..c.sha.len().min(10)],
            c.subjects
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n"),
            c.files
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n"),
            c.patch
        ));
    }
    if changes.iter().all(|c| c.files.is_empty()) {
        task.push_str("\nThe run changed no files.\n");
    }
    task
}

/// What the entry says when the scribe didn't deliver.
fn fallback_summary(input: &ScribeInput, changes: &[RepoChange]) -> String {
    let mut out = format!("## What changed\n\n{}\n", input.title);
    let subjects: Vec<&String> = changes.iter().flat_map(|c| &c.subjects).collect();
    for s in subjects {
        out.push_str(&format!("- {s}\n"));
    }
    out.push_str("\n## Files\n\n");
    let files: Vec<String> = changes
        .iter()
        .flat_map(|c| c.files.iter().map(move |f| format!("- {}/{f}", c.repo)))
        .collect();
    if files.is_empty() {
        out.push_str("none\n");
    } else {
        out.push_str(&files.join("\n"));
        out.push('\n');
    }
    if !input.issues_found.is_empty() || !input.issues_fixed.is_empty() {
        out.push_str("\n## Issues found and fixed\n\n");
        for i in input.issues_found.iter().chain(&input.issues_fixed) {
            out.push_str(&issue_line(i));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use db::models::issue_found::{IssueCategory, IssueFoundStatus, IssueSeverity};

    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn found(description: &str) -> IssueFound {
        IssueFound {
            id: Uuid::new_v4(),
            issue_id: Uuid::new_v4(),
            run_id: None,
            checkpoint: Some(0),
            file: Some("src/a.rs".into()),
            line: Some(3),
            severity: IssueSeverity::High,
            category: IssueCategory::EdgeCase,
            description: description.into(),
            status: IssueFoundStatus::Open,
            fixed_round: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn input(kind: ChangelogKind, root: PathBuf, commits: BTreeMap<String, String>) -> ScribeInput {
        ScribeInput {
            issue_id: Uuid::new_v4(),
            run_id: None,
            kind,
            title: "Subtask 1: Add login".into(),
            what: "Add a login form".into(),
            commits,
            ticket_root: root,
            issues_fixed: vec![],
            issues_found: vec![found("empty password accepted")],
        }
    }

    #[test]
    fn collects_files_and_patch_of_the_merge_commit_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app");
        std::fs::create_dir(&dir).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("old.rs"), "old\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("login.rs"), "fn login() {}\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "KAN-1: Merge subtask 1"]);
        let sha = String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let commits = BTreeMap::from([("app".to_string(), sha)]);
        let changes = collect_changes(tmp.path(), &commits);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].files, ["login.rs"]);
        assert_eq!(changes[0].subjects, ["KAN-1: Merge subtask 1"]);
        assert!(changes[0].patch.contains("+fn login() {}"));
        assert!(!changes[0].patch.contains("old"));

        let input = input(ChangelogKind::Coder, tmp.path().to_path_buf(), commits);
        let task = scribe_task("KAN-1", "Login", &input, &changes);
        assert!(task.contains("## Issues found and fixed"));
        assert!(task.contains("+fn login() {}"));
        assert!(task.contains("empty password accepted"));

        let fallback = fallback_summary(&input, &changes);
        assert!(fallback.contains("- app/login.rs"));
        assert!(fallback.contains("- KAN-1: Merge subtask 1"));
        assert!(fallback.contains("[high] empty password accepted (src/a.rs:3)"));
    }

    #[test]
    fn cut_respects_char_boundaries() {
        assert_eq!(cut("äöü", 3), "ä");
        assert_eq!(cut("abc", 10), "abc");
    }
}
