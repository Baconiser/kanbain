use axum::{Json, extract::State, response::Json as ResponseJson};
use db::models::{
    issue::Issue,
    project::Project,
    requests::{
        CreateAndStartWorkspaceRequest, CreateAndStartWorkspaceResponse, CreateWorkspaceApiRequest,
    },
    workspace::{CreateWorkspace, Workspace},
};
use deployment::Deployment;
use services::services::container::ContainerService;
use utils::{response::ApiResponse, ticket_id::ticket_branch_name};
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

pub(crate) async fn create_workspace_record(
    deployment: &DeploymentImpl,
    name: Option<String>,
    issue_id: Option<Uuid>,
) -> Result<Workspace, ApiError> {
    let pool = &deployment.db().pool;
    let workspace_id = Uuid::new_v4();

    // A workspace started from a ticket gets the ticket branch,
    // `kanbain/<ID>-<slug>` (D16), numbered when another workspace has it.
    let git_branch_name = match issue_id {
        Some(issue_id) => {
            let issue = Issue::find_by_id(pool, issue_id)
                .await?
                .ok_or_else(|| ApiError::BadRequest("Linked ticket not found".to_string()))?;
            let project = Project::find_by_id(pool, issue.project_id)
                .await?
                .ok_or_else(|| ApiError::BadRequest("Ticket project not found".to_string()))?;
            let base = ticket_branch_name(&issue.display_id(&project.id_prefix), &issue.title);
            let mut branch = base.clone();
            let mut n = 2;
            while Workspace::branch_in_use(pool, &branch).await? {
                branch = format!("{base}-{n}");
                n += 1;
            }
            branch
        }
        None => {
            let branch_label = name
                .as_deref()
                .filter(|branch_label| !branch_label.is_empty())
                .unwrap_or("workspace");
            deployment
                .container()
                .git_branch_from_workspace(&workspace_id, branch_label)
                .await
        }
    };

    let workspace = Workspace::create(
        pool,
        &CreateWorkspace {
            branch: git_branch_name,
            name: name.filter(|workspace_name| !workspace_name.is_empty()),
            issue_id,
        },
        workspace_id,
    )
    .await?;

    Ok(workspace)
}

pub async fn create_workspace(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateWorkspaceApiRequest>,
) -> Result<ResponseJson<ApiResponse<Workspace>>, ApiError> {
    let workspace = create_workspace_record(&deployment, payload.name, None).await?;

    Ok(ResponseJson(ApiResponse::success(workspace)))
}

fn normalize_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub async fn create_and_start_workspace(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateAndStartWorkspaceRequest>,
) -> Result<ResponseJson<ApiResponse<CreateAndStartWorkspaceResponse>>, ApiError> {
    let CreateAndStartWorkspaceRequest {
        name,
        repos,
        linked_issue,
        executor_config,
        prompt,
        attachment_ids,
    } = payload;

    let workspace_prompt = normalize_prompt(&prompt).ok_or_else(|| {
        ApiError::BadRequest(
            "A workspace prompt is required. Provide a non-empty `prompt`.".to_string(),
        )
    })?;

    if repos.is_empty() {
        return Err(ApiError::BadRequest(
            "At least one repository is required".to_string(),
        ));
    }

    let mut managed_workspace = deployment
        .workspace_manager()
        .load_managed_workspace(
            create_workspace_record(&deployment, name, linked_issue.map(|l| l.issue_id)).await?,
        )
        .await?;

    for repo in &repos {
        managed_workspace
            .add_repository(repo, deployment.git())
            .await
            .map_err(ApiError::from)?;
    }

    if let Some(ids) = &attachment_ids {
        managed_workspace.associate_attachments(ids).await?;
    }

    let workspace = managed_workspace.workspace.clone();
    tracing::info!("Created workspace {}", workspace.id);

    let execution_process = deployment
        .container()
        .start_workspace(&workspace, executor_config.clone(), workspace_prompt)
        .await?;

    Ok(ResponseJson(ApiResponse::success(
        CreateAndStartWorkspaceResponse {
            workspace,
            execution_process,
        },
    )))
}
