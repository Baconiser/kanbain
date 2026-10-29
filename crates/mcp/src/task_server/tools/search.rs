use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::McpServer;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct SearchKnowledgeRequest {
    #[schemars(description = "What you are looking for, in words or identifiers")]
    query: String,
    #[schemars(description = "Project to search; all projects when omitted")]
    project_id: Option<Uuid>,
    #[schemars(description = "Ticket whose branch changes (overlay) to include")]
    issue_id: Option<Uuid>,
    #[schemars(
        description = "Limit to these kinds: code, ticket, comment, attachment, prior_work, run"
    )]
    kinds: Option<Vec<String>>,
    #[schemars(description = "Maximum results (default 8)")]
    limit: Option<u32>,
}

#[tool_router(router = search_tools_router, vis = "pub")]
impl McpServer {
    #[tool(
        description = "Search KanbAIn's index of code, tickets, comments, prior work and agent runs by meaning and keywords. Returns locations (repo, path, line range, symbol; or ticket) with short excerpts, best first. Read only the ranges you need."
    )]
    async fn search_knowledge(
        &self,
        Parameters(request): Parameters<SearchKnowledgeRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let url = self.url("/api/search");
        let body = json!({
            "query": request.query,
            "project_id": request.project_id,
            "all_projects": request.project_id.is_none(),
            "issue_id": request.issue_id,
            "kinds": request.kinds,
            "limit": request.limit.unwrap_or(8).clamp(1, 20),
        });
        let results: Value = match self.send_json(self.client.post(&url).json(&body)).await {
            Ok(r) => r,
            Err(e) => return Ok(Self::tool_error(e)),
        };
        let tickets = results.get("tickets").cloned().unwrap_or(Value::Null);
        let repos = results.get("repos").cloned().unwrap_or(Value::Null);
        let hits: Vec<Value> = results
            .get("hits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|hit| {
                let ticket = hit
                    .get("issue_id")
                    .and_then(Value::as_str)
                    .and_then(|id| tickets.get(id))
                    .map(|t| json!({"id": t["internal_id"], "title": t["title"], "status": t["status"]}));
                let repo = hit
                    .get("repo_id")
                    .and_then(Value::as_str)
                    .and_then(|id| repos.get(id))
                    .cloned();
                json!({
                    "kind": hit["kind"],
                    "ticket": ticket,
                    "repo": repo,
                    "path": hit["path"],
                    "start_line": hit["start_line"],
                    "end_line": hit["end_line"],
                    "symbol": hit["symbol"],
                    "excerpt": hit["excerpt"],
                })
            })
            .collect();
        McpServer::success(&json!({ "results": hits, "semantic": results["semantic"] }))
    }
}
