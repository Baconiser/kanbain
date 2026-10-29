//! The `search_knowledge` tool for sandboxed runs (E22), served by the model
//! proxy's `/mcp` endpoint. A run searches its ticket's project, with the
//! ticket's overlay, and gets locations with capped excerpts (D43). Every
//! call is logged as a run event with its result count and excerpt bytes
//! (the D46 measurement).

use std::collections::HashMap;

use async_trait::async_trait;
use db::{
    DBService,
    models::{
        execution_process::ExecutionProcess,
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::Issue,
        project::Project,
        repo::Repo,
    },
};
use embeddings::{
    SearchService,
    service::{SearchHit, SearchRequest},
};
use model_proxy::mcp::RunTools;
use serde_json::{Value, json};
use uuid::Uuid;

pub const SEARCH_TOOL: &str = "search_knowledge";
const DEFAULT_LIMIT: u32 = 8;
const MAX_LIMIT: u32 = 20;

pub struct KnowledgeTools {
    db: DBService,
    search: SearchService,
}

impl KnowledgeTools {
    pub fn new(db: DBService, search: SearchService) -> Self {
        Self { db, search }
    }
}

/// The text the agent gets: one block per hit, location first.
pub fn format_hits(
    query: &str,
    hits: &[SearchHit],
    tickets: &HashMap<Uuid, String>,
    repos: &HashMap<Uuid, String>,
) -> String {
    if hits.is_empty() {
        return format!("No results for \"{query}\". Explore the code as usual.");
    }
    let mut out = format!("{} results for \"{query}\":\n", hits.len());
    for (i, hit) in hits.iter().enumerate() {
        let location = if hit.kind == "code" {
            let repo = hit
                .repo_id
                .and_then(|r| repos.get(&r).cloned())
                .unwrap_or_default();
            let lines = match (hit.start_line, hit.end_line) {
                (Some(a), Some(b)) => format!(":{a}-{b}"),
                _ => String::new(),
            };
            let symbol = hit
                .symbol
                .as_deref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default();
            format!(
                "code · {repo}/{}{lines}{symbol}",
                hit.path.as_deref().unwrap_or("")
            )
        } else {
            let ticket = hit
                .issue_id
                .and_then(|id| tickets.get(&id).cloned())
                .unwrap_or_default();
            format!("{} · {ticket}", hit.kind)
        };
        out.push_str(&format!("\n{}. {location}\n", i + 1));
        for line in hit.excerpt.lines() {
            out.push_str("   ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("\nRead only the line ranges you need.");
    out
}

#[async_trait]
impl RunTools for KnowledgeTools {
    fn definitions(&self) -> Vec<Value> {
        vec![json!({
            "name": SEARCH_TOOL,
            "description": "Search this project's code, tickets, comments, prior work and earlier \
                agent runs by meaning and keywords. Returns locations (repo/path:lines and symbol \
                for code; ticket ids for tickets) with short excerpts, best first. Use it before \
                exploring with Grep, Glob or Read, then read only the ranges you need.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you are looking for, in words or identifiers."
                    },
                    "kinds": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["code", "ticket", "comment", "attachment", "prior_work", "run"]
                        },
                        "description": "Limit the results to these kinds."
                    },
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT}
                },
                "required": ["query"]
            }
        })]
    }

    async fn call(&self, run_id: Uuid, name: &str, arguments: Value) -> Result<String, String> {
        if name != SEARCH_TOOL {
            return Err(format!("Unknown tool {name}"));
        }
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or("`query` is required")?
            .to_string();
        let kinds: Option<Vec<String>> = arguments.get("kinds").and_then(|k| {
            serde_json::from_value::<Vec<String>>(k.clone())
                .ok()
                .filter(|k| !k.is_empty())
        });
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map(|l| l.clamp(1, MAX_LIMIT as u64) as u32)
            .unwrap_or(DEFAULT_LIMIT);
        let pool = &self.db.pool;
        let run = ExecutionProcess::find_by_id(pool, run_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("This run is not known to KanbAIn")?;
        let issue = match run.issue_id {
            Some(id) => Issue::find_by_id(pool, id)
                .await
                .map_err(|e| e.to_string())?,
            None => None,
        };
        let response = self
            .search
            .search(&SearchRequest {
                query: query.clone(),
                project_id: issue.as_ref().map(|i| i.project_id),
                issue_id: issue.as_ref().map(|i| i.id),
                kinds: kinds.clone(),
                limit: Some(limit),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let mut tickets = HashMap::new();
        let mut repos = HashMap::new();
        for hit in &response.hits {
            if let Some(id) = hit.issue_id
                && !tickets.contains_key(&id)
                && let Ok(Some(ticket)) = Issue::find_by_id(pool, id).await
            {
                let prefix = Project::find_by_id(pool, ticket.project_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|p| p.id_prefix)
                    .unwrap_or_else(|| "KAN".into());
                tickets.insert(
                    id,
                    format!("{} \"{}\"", ticket.display_id(&prefix), ticket.title),
                );
            }
            if let Some(id) = hit.repo_id
                && !repos.contains_key(&id)
                && let Ok(Some(repo)) = Repo::find_by_id(pool, id).await
            {
                repos.insert(id, repo.name);
            }
        }
        let text = format_hits(&query, &response.hits, &tickets, &repos);
        let excerpt_bytes: usize = response.hits.iter().map(|h| h.excerpt.len()).sum();
        let _ = ExecutionProcessEvent::create(
            pool,
            run_id,
            ExecutionProcessEventType::ToolCall,
            &json!({
                "tool": SEARCH_TOOL,
                "query": query,
                "kinds": kinds,
                "results": response.hits.len(),
                "excerpt_bytes": excerpt_bytes,
                "response_bytes": text.len(),
                "semantic": response.semantic,
                "took_ms": response.took_ms,
            }),
        )
        .await;
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: &str) -> SearchHit {
        SearchHit {
            kind: kind.into(),
            score: 0.1,
            vector_rank: Some(1),
            keyword_rank: None,
            excerpt: "fn create() {\n    1\n}".into(),
            project_id: String::new(),
            issue_id: None,
            comment_id: None,
            attachment_id: None,
            execution_process_id: None,
            repo_id: None,
            path: Some("src/issue.rs".into()),
            start_line: Some(10),
            end_line: Some(12),
            symbol: Some("impl Issue › fn create".into()),
        }
    }

    #[test]
    fn formats_locations_first() {
        let text = format_hits(
            "ticket numbers",
            &[hit("code")],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(text.contains("1. code · /src/issue.rs:10-12 · impl Issue › fn create"));
        assert!(text.contains("   fn create() {"));
        assert!(format_hits("x", &[], &HashMap::new(), &HashMap::new()).starts_with("No results"));
    }
}
