//! Changelog export (Phase 7, D11): a ticket's or a project's changelog
//! entries as Markdown. Nothing goes into the project repos; the user
//! downloads the file and does with it what they like.

use db::models::{
    changelog_entry::{ChangelogEntry, ChangelogKind},
    issue::Issue,
    issue_found::{IssueFound, IssueFoundStatus},
};

/// Files listed per entry; the rest are counted.
const MAX_FILES_LISTED: usize = 20;

/// One ticket's part of an export.
pub struct TicketChangelog<'a> {
    /// e.g. `KAN-12`
    pub id: &'a str,
    pub issue: &'a Issue,
    pub status: &'a str,
    pub entries: &'a [ChangelogEntry],
    pub issues_found: &'a [IssueFound],
}

fn kind_label(kind: ChangelogKind) -> &'static str {
    match kind {
        ChangelogKind::Coder => "Coder",
        ChangelogKind::Tester => "Tester",
        ChangelogKind::Fix => "Fix",
        ChangelogKind::Merge => "Merge",
    }
}

/// Pushes the scribe's sections one heading level down, so its `##`
/// headings nest under the entry's `###`.
fn nest(summary: &str, levels: usize) -> String {
    let extra = "#".repeat(levels);
    let mut in_fence = false;
    summary
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            }
            if !in_fence && line.starts_with('#') {
                format!("{extra}{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn ticket_body(t: &TicketChangelog<'_>, level: usize) -> String {
    let h = "#".repeat(level);
    let mut md = String::new();
    if let Some(external) = &t.issue.external_id
        && external != t.id
    {
        md.push_str(&format!("External ID: {external}  \n"));
    }
    md.push_str(&format!("Status: {}\n", t.status));

    if t.entries.is_empty() {
        md.push_str("\n_No changelog entries yet._\n");
    }
    for entry in t.entries {
        md.push_str(&format!(
            "\n{h}# {} · {} ({})\n\n",
            kind_label(entry.kind),
            entry.title,
            entry.created_at.format("%Y-%m-%d %H:%M")
        ));
        md.push_str(nest(entry.summary.trim(), level).trim());
        md.push('\n');
        if !entry.commit_refs.0.is_empty() {
            let refs: Vec<String> = entry
                .commit_refs
                .0
                .iter()
                .map(|c| format!("`{}@{}`", c.repo, short(&c.sha)))
                .collect();
            md.push_str(&format!("\nCommits: {}\n", refs.join(", ")));
        }
        // From git, so it holds even when the scribe's prose doesn't.
        if !entry.files.0.is_empty() {
            let shown: Vec<String> = entry
                .files
                .0
                .iter()
                .take(MAX_FILES_LISTED)
                .map(|f| format!("`{f}`"))
                .collect();
            let more = entry.files.0.len().saturating_sub(MAX_FILES_LISTED);
            md.push_str(&format!(
                "Files: {}{}\n",
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
    }

    let fixed: Vec<&IssueFound> = t
        .issues_found
        .iter()
        .filter(|i| i.status == IssueFoundStatus::Fixed)
        .collect();
    let other: Vec<&IssueFound> = t
        .issues_found
        .iter()
        .filter(|i| i.status != IssueFoundStatus::Fixed)
        .collect();
    if !t.issues_found.is_empty() {
        md.push_str(&format!(
            "\n{h}# Issues found\n\n{} fixed, {} open or won't fix.\n\n",
            fixed.len(),
            other.len()
        ));
        for i in fixed.iter().chain(&other) {
            let at = match (&i.file, i.line) {
                (Some(f), Some(l)) => format!(" (`{f}:{l}`)"),
                (Some(f), None) => format!(" (`{f}`)"),
                _ => String::new(),
            };
            md.push_str(&format!(
                "- **{}** [{}] {}{at}\n",
                i.status, i.severity, i.description
            ));
        }
    }
    md
}

/// One ticket as a Markdown document.
pub fn render_ticket(t: &TicketChangelog<'_>) -> String {
    format!("# {}: {}\n\n{}", t.id, t.issue.title, ticket_body(t, 2))
}

/// A project's tickets, newest change first, as one Markdown document.
/// Tickets without entries are left out: a changelog lists changes.
pub fn render_project(project_name: &str, tickets: &[TicketChangelog<'_>]) -> String {
    let mut with_entries: Vec<&TicketChangelog<'_>> =
        tickets.iter().filter(|t| !t.entries.is_empty()).collect();
    with_entries.sort_by_key(|t| std::cmp::Reverse(t.entries.iter().map(|e| e.created_at).max()));
    let mut md = format!("# Changelog: {project_name}\n");
    if with_entries.is_empty() {
        md.push_str("\n_No changelog entries yet._\n");
    }
    for t in with_entries {
        md.push_str(&format!("\n## {}: {}\n\n", t.id, t.issue.title));
        md.push_str(&ticket_body(t, 3));
    }
    md
}

/// A file name for the download: `KAN-12-changelog.md`.
pub fn file_name(stem: &str) -> String {
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{}-changelog.md", safe.trim_matches('-'))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use db::models::{
        changelog_entry::CommitRef,
        issue::IssueMode,
        issue_found::{IssueCategory, IssueSeverity},
    };
    use sqlx::types::Json;
    use uuid::Uuid;

    use super::*;

    fn issue(title: &str) -> Issue {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "project_id": Uuid::new_v4(), "number": 12,
            "title": title, "description": null, "status_id": Uuid::new_v4(),
            "priority": null, "sort_order": 0.0, "parent_issue_id": null,
            "parent_issue_sort_order": null,
            "external_id": "PROJ-7", "mode": IssueMode::Ask, "fix_round": 0,
            "budget_usd": null, "completed_at": null,
            "created_at": Utc::now(), "updated_at": Utc::now(),
        }))
        .unwrap()
    }

    fn entry(kind: ChangelogKind, title: &str, minute: u32) -> ChangelogEntry {
        ChangelogEntry {
            id: Uuid::new_v4(),
            issue_id: Uuid::new_v4(),
            run_id: None,
            job_id: None,
            kind,
            title: title.into(),
            summary: "## What changed\n\nAdded the form.\n\n```md\n# not a heading\n```".into(),
            files: Json(vec!["app/login.rs".into()]),
            commit_refs: Json(vec![CommitRef {
                repo: "app".into(),
                sha: "0123456789abcdef".into(),
            }]),
            issues_fixed: Json(vec![]),
            issues_found: Json(vec![]),
            scribe_error: None,
            created_at: Utc.with_ymd_and_hms(2026, 11, 4, 10, minute, 0).unwrap(),
        }
    }

    fn found(status: IssueFoundStatus) -> IssueFound {
        IssueFound {
            id: Uuid::new_v4(),
            issue_id: Uuid::new_v4(),
            run_id: None,
            checkpoint: Some(0),
            file: Some("app/login.rs".into()),
            line: Some(4),
            severity: IssueSeverity::Medium,
            category: IssueCategory::Logic,
            description: "Empty password accepted".into(),
            status,
            fixed_round: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn ticket_export_nests_headings_but_not_code() {
        let issue = issue("Login form");
        let entries = [entry(ChangelogKind::Coder, "Subtask 1: Form", 0)];
        let issues = [
            found(IssueFoundStatus::Fixed),
            found(IssueFoundStatus::Open),
        ];
        let md = render_ticket(&TicketChangelog {
            id: "KAN-12",
            issue: &issue,
            status: "Review",
            entries: &entries,
            issues_found: &issues,
        });
        assert!(md.starts_with("# KAN-12: Login form\n"));
        assert!(md.contains("External ID: PROJ-7"));
        assert!(md.contains("### Coder · Subtask 1: Form (2026-11-04 10:00)"));
        assert!(md.contains("#### What changed"));
        assert!(md.contains("\n# not a heading\n"), "{md}");
        assert!(md.contains("Commits: `app@0123456789`\nFiles: `app/login.rs`\n"));
        assert!(md.contains("1 fixed, 1 open or won't fix."));
        assert!(md.contains("- **fixed** [medium] Empty password accepted (`app/login.rs:4`)"));
    }

    #[test]
    fn project_export_skips_empty_tickets_and_sorts_newest_first() {
        let (a, b, c) = (issue("Old"), issue("New"), issue("Untouched"));
        let old = [entry(ChangelogKind::Merge, "Merged", 1)];
        let new = [entry(ChangelogKind::Fix, "Fixes", 30)];
        let md = render_project(
            "Shop",
            &[
                TicketChangelog {
                    id: "KAN-1",
                    issue: &a,
                    status: "Done",
                    entries: &old,
                    issues_found: &[],
                },
                TicketChangelog {
                    id: "KAN-2",
                    issue: &b,
                    status: "Review",
                    entries: &new,
                    issues_found: &[],
                },
                TicketChangelog {
                    id: "KAN-3",
                    issue: &c,
                    status: "Backlog",
                    entries: &[],
                    issues_found: &[],
                },
            ],
        );
        assert!(md.starts_with("# Changelog: Shop\n"));
        assert!(md.find("KAN-2: New").unwrap() < md.find("KAN-1: Old").unwrap());
        assert!(!md.contains("Untouched"));
        assert!(md.contains("#### Fix · Fixes"));
        assert!(md.contains("##### What changed"));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("KAN-12"), "KAN-12-changelog.md");
        assert_eq!(file_name("My Shop/2"), "My-Shop-2-changelog.md");
    }
}
