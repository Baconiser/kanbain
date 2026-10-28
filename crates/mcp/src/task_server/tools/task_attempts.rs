use db::models::requests::{
    CreateAndStartWorkspaceRequest, CreateAndStartWorkspaceResponse, WorkspaceRepoInput,
};
use executors::profile::ExecutorConfig;
use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::McpServer;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct McpWorkspaceRepoInput {
    #[schemars(description = "The repository ID")]
    repo_id: Uuid,
    #[schemars(description = "The branch for this repository")]
    branch: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StartWorkspaceRequest {
    #[schemars(description = "Name for the workspace")]
    name: String,
    #[schemars(description = "Prompt for the first workspace session")]
    prompt: Option<String>,
    #[schemars(description = "The coding agent executor to run ('CLAUDE_CODE', 'OPENCODE')")]
    executor: String,
    #[schemars(description = "Optional executor variant, if needed")]
    variant: Option<String>,
    #[schemars(description = "Repository selection for the workspace")]
    repositories: Vec<McpWorkspaceRepoInput>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct StartWorkspaceResponse {
    workspace_id: String,
}

#[tool_router(router = task_attempts_tools_router, vis = "pub")]
impl McpServer {
    #[tool(description = "Create a new workspace and start its first session.")]
    async fn start_workspace(
        &self,
        Parameters(StartWorkspaceRequest {
            name,
            prompt,
            executor,
            variant,
            repositories,
        }): Parameters<StartWorkspaceRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        if repositories.is_empty() {
            return Self::err("At least one repository must be specified.", None::<&str>);
        }

        let executor_trimmed = executor.trim();
        if executor_trimmed.is_empty() {
            return Self::err("Executor must not be empty.", None::<&str>);
        }

        let prompt = prompt.and_then(|prompt| {
            let trimmed = prompt.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        let base_executor = match Self::parse_executor_agent(executor_trimmed) {
            Ok(exec) => exec,
            Err(_) => {
                return Self::err(
                    format!("Unknown executor '{executor_trimmed}'."),
                    None::<String>,
                );
            }
        };

        let variant = variant.and_then(|v| {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        let workspace_repos: Vec<WorkspaceRepoInput> = repositories
            .into_iter()
            .map(|r| WorkspaceRepoInput {
                repo_id: r.repo_id,
                target_branch: r.branch,
            })
            .collect();

        let Some(workspace_prompt) = prompt else {
            return Self::err("Provide a non-empty `prompt`.", None::<&str>);
        };

        let create_and_start_payload = CreateAndStartWorkspaceRequest {
            name: Some(name.clone()),
            repos: workspace_repos,
            linked_issue: None,
            executor_config: ExecutorConfig {
                executor: base_executor,
                variant,
                model_id: None,
                agent_id: None,
                reasoning_id: None,
                permission_policy: None,
                agent_definition: None,
            },
            prompt: workspace_prompt,
            attachment_ids: None,
        };

        let create_and_start_url = self.url("/api/workspaces/start");
        let create_and_start_response: CreateAndStartWorkspaceResponse = match self
            .send_json(
                self.client
                    .post(&create_and_start_url)
                    .json(&create_and_start_payload),
            )
            .await
        {
            Ok(response) => response,
            Err(e) => return Ok(Self::tool_error(e)),
        };

        let response = StartWorkspaceResponse {
            workspace_id: create_and_start_response.workspace.id.to_string(),
        };

        McpServer::success(&response)
    }
}
