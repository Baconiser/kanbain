//! Text sources of a ticket (E15, D44): the ticket, its dropped original,
//! comments, attachments' extracted text, the prior-work summary, the
//! newest plan (E6, Phase 4), summaries of finished runs and the issues the
//! tester found (E6, Phase 6). Every chunk header names the ticket.

use std::collections::HashSet;

use db::models::{
    attachment_text::AttachmentText, coding_agent_turn::CodingAgentTurn,
    execution_process::ExecutionProcess, execution_process_repo_state::ExecutionProcessRepoState,
    file::File, issue::Issue, issue_comment::IssueComment, issue_description::IssueDescription,
    issue_found::IssueFound, issue_history::IssueHistory, plan::Plan, project::Project, repo::Repo,
};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::chunk::text::{Chunk, TEXT_BUDGET, TEXT_OVERLAP, chunk_text, hash_text};

pub const KIND_TICKET: &str = "ticket";
pub const KIND_COMMENT: &str = "comment";
pub const KIND_ATTACHMENT: &str = "attachment";
pub const KIND_PRIOR_WORK: &str = "prior_work";
pub const KIND_RUN: &str = "run";
pub const KIND_PLAN: &str = "plan";
pub const KIND_ISSUE_FOUND: &str = "issue_found";

/// Text kinds this phase indexes; E21 lists more for later phases.
pub const TEXT_KINDS: &[&str] = &[
    KIND_TICKET,
    KIND_COMMENT,
    KIND_ATTACHMENT,
    KIND_PRIOR_WORK,
    KIND_RUN,
    KIND_PLAN,
    KIND_ISSUE_FOUND,
];

/// One source to store.
#[derive(Debug, Clone)]
pub struct TextSource {
    pub kind: &'static str,
    pub ref_id: String,
    pub content_hash: String,
    pub chunks: Vec<Chunk>,
}

/// Longest run summary text.
const MAX_RUN_SUMMARY_CHARS: usize = 4000;

fn source(kind: &'static str, ref_id: String, header: &str, body: &str) -> Option<TextSource> {
    let chunks = chunk_text(header, body, TEXT_BUDGET, TEXT_OVERLAP);
    if chunks.is_empty() {
        return None;
    }
    Some(TextSource {
        kind,
        content_hash: hash_text(&format!("{header}\n{body}")),
        ref_id,
        chunks,
    })
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

/// `KAN-12 · PROJ-123 · Title`
pub fn ticket_header(issue: &Issue, project: &Project) -> String {
    let internal = issue.internal_id(&project.id_prefix);
    match &issue.external_id {
        Some(external) => format!("{internal} · {external} · {}", issue.title),
        None => format!("{internal} · {}", issue.title),
    }
}

/// Files a run changed, from its repos' head before and after.
async fn run_files(pool: &SqlitePool, execution_id: Uuid) -> Vec<String> {
    let Ok(states) =
        ExecutionProcessRepoState::find_by_execution_process_id(pool, execution_id).await
    else {
        return vec![];
    };
    let mut files = Vec::new();
    for state in states {
        let (Some(before), Some(after)) = (state.before_head_commit, state.after_head_commit)
        else {
            continue;
        };
        if before == after {
            continue;
        }
        let Ok(Some(repo)) = Repo::find_by_id(pool, state.repo_id).await else {
            continue;
        };
        let path = repo.path.clone();
        let range = format!("{before}..{after}");
        let out = tokio::task::spawn_blocking(move || {
            git::GitCli::new().git(&path, ["diff", "--name-only", range.as_str()])
        })
        .await;
        if let Ok(Ok(out)) = out {
            files.extend(
                out.lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| format!("{}/{}", repo.name, l.trim())),
            );
        }
    }
    files
}

/// A finished run as one short summary (D44): task, agent, outcome, files,
/// cost and the agent's final message. Never the raw transcript.
async fn run_summary(pool: &SqlitePool, run: &ExecutionProcess) -> String {
    let turn = CodingAgentTurn::find_by_execution_process_id(pool, run.id)
        .await
        .ok()
        .flatten();
    let mut text = String::from("Agent run");
    if let Some(agent) = &run.agent_def_id {
        text.push_str(&format!(" by {agent}"));
    }
    text.push_str(&format!(": {:?}", run.status).to_lowercase());
    let minutes = run
        .completed_at
        .map(|end| (end - run.started_at).num_seconds().max(0) / 60)
        .unwrap_or(0);
    text.push_str(&format!(
        ", {minutes} min, {} tokens, ${:.2}.\n",
        run.tokens_in + run.tokens_out,
        run.cost_usd
    ));
    if let Some(prompt) = turn.as_ref().and_then(|t| t.prompt.as_deref()) {
        text.push_str(&format!("\nTask: {}\n", cut(prompt.trim(), 800)));
    }
    let files = run_files(pool, run.id).await;
    if !files.is_empty() {
        text.push_str(&format!("\nFiles changed: {}\n", files.join(", ")));
    }
    if let Some(summary) = turn.as_ref().and_then(|t| t.summary.as_deref()) {
        text.push_str(&format!("\nOutcome: {}\n", summary.trim()));
    }
    cut(&text, MAX_RUN_SUMMARY_CHARS).to_string()
}

/// Every text source of a ticket.
pub async fn ticket_sources(
    pool: &SqlitePool,
    issue: &Issue,
    project: &Project,
) -> Result<Vec<TextSource>, sqlx::Error> {
    let header = ticket_header(issue, project);
    let issue_ref = issue.id.to_string();
    let mut sources = Vec::new();

    let description = issue.description.clone().unwrap_or_default();
    sources.extend(source(
        KIND_TICKET,
        issue_ref.clone(),
        &header,
        &format!("# {}\n\n{}", issue.title, description),
    ));
    // The dropped original, when the description no longer says the same.
    if let Some(original) = IssueDescription::find_by_issue(pool, issue.id)
        .await?
        .into_iter()
        .find_map(|v| v.original_text)
        .filter(|o| o.trim() != description.trim() && !o.trim().is_empty())
    {
        sources.extend(source(
            KIND_TICKET,
            format!("{issue_ref}:original"),
            &header,
            &format!("Original ticket text:\n\n{original}"),
        ));
    }

    for comment in IssueComment::find_by_issue(pool, issue.id).await? {
        let author = if comment.author == "user" {
            "the user"
        } else {
            "an agent"
        };
        sources.extend(source(
            KIND_COMMENT,
            comment.id.to_string(),
            &header,
            &format!("Comment by {author}:\n\n{}", comment.message),
        ));
    }

    for file in File::find_by_issue_id(pool, issue.id).await? {
        if let Some(text) = AttachmentText::find(pool, file.id).await?
            && text.error.is_none()
        {
            sources.extend(source(
                KIND_ATTACHMENT,
                format!("{issue_ref}:{}", file.id),
                &header,
                &format!("Attachment {}:\n\n{}", file.original_name, text.text),
            ));
        }
    }

    if let Some(prior) = IssueHistory::find(pool, issue.id)
        .await?
        .and_then(|h| h.prior_work)
    {
        sources.extend(source(
            KIND_PRIOR_WORK,
            issue_ref.clone(),
            &header,
            &format!("Prior work from the git history:\n\n{prior}"),
        ));
    }

    // Only the newest version: older ones would crowd out other tickets.
    if let Some(plan) = Plan::latest(pool, issue.id).await? {
        sources.extend(source(
            KIND_PLAN,
            issue_ref.clone(),
            &header,
            &format!(
                "Plan v{} ({}):\n\n{}",
                plan.version,
                plan.status,
                plan.plan.0.to_markdown()
            ),
        ));
    }

    // Every issue the tester found, fixed ones too: a later run touching
    // the same file sees what broke there before.
    for found in IssueFound::find_by_issue(pool, issue.id).await? {
        let at = match (&found.file, found.line) {
            (Some(f), Some(l)) => format!(" in {f}:{l}"),
            (Some(f), None) => format!(" in {f}"),
            _ => String::new(),
        };
        sources.extend(source(
            KIND_ISSUE_FOUND,
            found.id.to_string(),
            &header,
            &format!(
                "Issue found by the tester ({} severity, {}, {}){at}:\n\n{}",
                found.severity,
                found.category.to_string().replace('_', " "),
                found.status,
                found.description
            ),
        ));
    }

    let run_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM execution_processes WHERE issue_id = ? AND run_reason = 'codingagent' \
         AND status != 'running' ORDER BY started_at",
    )
    .bind(issue.id)
    .fetch_all(pool)
    .await?;
    for id in run_ids {
        if let Some(run) = ExecutionProcess::find_by_id(pool, id).await? {
            let summary = run_summary(pool, &run).await;
            sources.extend(source(KIND_RUN, id.to_string(), &header, &summary));
        }
    }

    // Unique refs per kind.
    let mut seen = HashSet::new();
    sources.retain(|s| seen.insert((s.kind, s.ref_id.clone())));
    Ok(sources)
}
