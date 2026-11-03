//! Ticket ids in text, branch names and commit messages (D16).

use regex::Regex;

/// Jira-style keys such as `PROJEKTNAME-123`, with or without brackets.
pub const DEFAULT_EXTERNAL_ID_PATTERN: &str = r"\[?[A-Z][A-Z0-9]*-\d+\]?";

/// Finds the first External ID in `text`.
///
/// `pattern` overrides the default regex; `key` (for example `PROJEKTNAME-`
/// or `PROJEKTNAME`) keeps only ids with that key. Ids using the project's
/// own internal prefix are never External IDs.
pub fn detect_external_id(
    text: &str,
    pattern: Option<&str>,
    key: Option<&str>,
    internal_prefix: Option<&str>,
) -> Option<String> {
    let pattern = pattern
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(DEFAULT_EXTERNAL_ID_PATTERN);
    let re = Regex::new(pattern)
        .or_else(|_| Regex::new(DEFAULT_EXTERNAL_ID_PATTERN))
        .ok()?;
    let key = key
        .map(|k| k.trim().trim_end_matches('-').to_ascii_uppercase())
        .filter(|k| !k.is_empty());
    let internal = internal_prefix.map(|p| format!("{}-", p.trim().to_ascii_uppercase()));

    re.find_iter(text)
        .map(|m| {
            m.as_str()
                .trim_matches(|c| c == '[' || c == ']')
                .to_string()
        })
        .filter(|id| !id.is_empty())
        .find(|id| {
            let upper = id.to_ascii_uppercase();
            let key_ok = key
                .as_ref()
                .is_none_or(|k| upper.starts_with(&format!("{k}-")));
            let not_internal = internal.as_ref().is_none_or(|p| !upper.starts_with(p));
            key_ok && not_internal
        })
}

fn slug(title: &str, max: usize) -> String {
    let lower = title.to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let cut: String = out.chars().take(max).collect();
    cut.trim_end_matches('-').to_string()
}

/// Characters git refuses in a branch name component.
fn branch_safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(|c| c == '-' || c == '.')
        .to_string()
}

/// The ticket branch, `kanbain/<ID>-<slug>`.
pub fn ticket_branch_name(id: &str, title: &str) -> String {
    let id = branch_safe(id);
    let slug = slug(title, 40);
    if slug.is_empty() {
        format!("kanbain/{id}")
    } else {
        format!("kanbain/{id}-{slug}")
    }
}

/// A subtask's branch, `<ticket branch>--<subtask id>`. Not `…/<n>` as the
/// plan sketched: git stores refs as files, so `kanbain/X-slug` and
/// `kanbain/X-slug/1` can't both exist.
pub fn subtask_branch_name(ticket_branch: &str, subtask_id: &str) -> String {
    let id = branch_safe(subtask_id);
    let id = if id.is_empty() {
        "subtask"
    } else {
        id.as_str()
    };
    format!("{ticket_branch}--{id}")
}

/// `<ID>: <message>`, unless the message already starts with the id.
pub fn ticket_commit_message(id: &str, message: &str) -> String {
    let message = message.trim();
    let starts_with_id = message
        .get(..id.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(id))
        && !message[id.len()..].starts_with(|c: char| c.is_ascii_alphanumeric());
    if starts_with_id {
        message.to_string()
    } else {
        format!("{id}: {message}")
    }
}

/// Bullets past this get summarised as "…and N more"; a merge commit is a
/// changelog, not a transcript.
const MAX_CHANGELOG_ITEMS: usize = 20;

/// Message for merging a finished workspace into its target branch:
/// `<ID>: <subject>`, a blank line, one bullet per piece of work, and a
/// trailer naming the workspace. Items that repeat the ticket id lose it,
/// duplicates and empties are dropped.
pub fn changelog_commit_message(
    id: Option<&str>,
    subject: &str,
    items: &[String],
    workspace_id: &str,
) -> String {
    let strip_id = |item: &str| -> String {
        let item = item.trim();
        match id {
            Some(id) => {
                let prefixed = ticket_commit_message(id, item);
                if prefixed == item {
                    // Already started with the id: drop it and the separator.
                    item[id.len()..]
                        .trim_start_matches([':', ' ', '-'])
                        .to_string()
                } else {
                    item.to_string()
                }
            }
            None => item.to_string(),
        }
    };
    let mut seen = std::collections::HashSet::new();
    let bullets: Vec<String> = items
        .iter()
        .map(|item| strip_id(item))
        .filter(|item| !item.is_empty() && seen.insert(item.to_lowercase()))
        .collect();

    let mut message = match id {
        Some(id) => ticket_commit_message(id, subject),
        None => subject.trim().to_string(),
    };
    if !bullets.is_empty() {
        message.push_str("\n\n");
        for bullet in bullets.iter().take(MAX_CHANGELOG_ITEMS) {
            message.push_str(&format!("- {bullet}\n"));
        }
        if bullets.len() > MAX_CHANGELOG_ITEMS {
            message.push_str(&format!(
                "- …and {} more\n",
                bullets.len() - MAX_CHANGELOG_ITEMS
            ));
        }
    } else {
        message.push('\n');
    }
    message.push_str(&format!("\nKanbAIn-Workspace: {workspace_id}\n"));
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_message_lists_work_without_repeating_the_id() {
        let items = vec![
            "KAN-12: Add the 2D pipeline".to_string(),
            "kan-12 - Wire up textures".to_string(),
            "Add the 2D pipeline".to_string(),
            "  ".to_string(),
            "Fix KAN-120 lookups".to_string(),
        ];
        assert_eq!(
            changelog_commit_message(Some("KAN-12"), "Render pipeline", &items, "ws-1"),
            "KAN-12: Render pipeline\n\n\
             - Add the 2D pipeline\n\
             - Wire up textures\n\
             - Fix KAN-120 lookups\n\
             \nKanbAIn-Workspace: ws-1\n"
        );
    }

    #[test]
    fn changelog_message_without_items_or_ticket() {
        assert_eq!(
            changelog_commit_message(None, "my-branch", &[], "ws-1"),
            "my-branch\n\nKanbAIn-Workspace: ws-1\n"
        );
    }

    #[test]
    fn changelog_message_caps_the_list() {
        let items: Vec<String> = (1..=25).map(|n| format!("Item {n}")).collect();
        let message = changelog_commit_message(Some("KAN-1"), "Big one", &items, "ws");
        assert!(message.contains("- Item 20\n- …and 5 more\n"));
        assert!(!message.contains("Item 21"));
    }

    #[test]
    fn detects_jira_keys() {
        assert_eq!(
            detect_external_id("Bug [PROJEKTNAME-123] login", None, None, None).as_deref(),
            Some("PROJEKTNAME-123")
        );
        assert_eq!(
            detect_external_id("see AB2-7 and CD-9", None, None, None).as_deref(),
            Some("AB2-7")
        );
        assert_eq!(
            detect_external_id("no id here, utf-8", None, None, None),
            None
        );
    }

    #[test]
    fn key_and_internal_prefix_filter() {
        let text = "Related: KAN-4, OTHER-1, [PROJ-12]";
        assert_eq!(
            detect_external_id(text, None, Some("PROJ-"), None).as_deref(),
            Some("PROJ-12")
        );
        assert_eq!(
            detect_external_id(text, None, Some("proj"), None).as_deref(),
            Some("PROJ-12")
        );
        assert_eq!(
            detect_external_id(text, None, None, Some("KAN")).as_deref(),
            Some("OTHER-1")
        );
    }

    #[test]
    fn custom_pattern_and_bad_pattern() {
        assert_eq!(
            detect_external_id("ticket #4521", Some(r"#\d+"), None, None).as_deref(),
            Some("#4521")
        );
        // An invalid override falls back to the default.
        assert_eq!(
            detect_external_id("PROJ-1", Some("(["), None, None).as_deref(),
            Some("PROJ-1")
        );
    }

    #[test]
    fn branch_and_commit() {
        assert_eq!(
            ticket_branch_name("PROJ-123", "Password reset: mail never arrives!"),
            "kanbain/PROJ-123-password-reset-mail-never-arrives"
        );
        assert_eq!(ticket_branch_name("#45", "  "), "kanbain/45");
        assert_eq!(
            subtask_branch_name("kanbain/PROJ-1-login", "2"),
            "kanbain/PROJ-1-login--2"
        );
        assert_eq!(
            subtask_branch_name("kanbain/PROJ-1-login", "a b/c"),
            "kanbain/PROJ-1-login--a-b-c"
        );
        assert_eq!(
            ticket_commit_message("PROJ-1", "Fix login"),
            "PROJ-1: Fix login"
        );
        assert_eq!(
            ticket_commit_message("PROJ-1", "proj-1: Fix login"),
            "proj-1: Fix login"
        );
        assert_eq!(
            ticket_commit_message("PROJ-1", "PROJ-12: other"),
            "PROJ-1: PROJ-12: other"
        );
    }
}
