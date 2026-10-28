use axum::{
    Json, Router,
    extract::{Path, Query, State, ws::Message},
    response::{IntoResponse, Json as ResponseJson},
    routing::{delete, get, post, put},
};
use db::models::{
    issue::{BulkUpdateIssueItem, CreateIssue, Issue},
    project::{CreateProject, Project, ProjectRepo, UpdateProject},
    project_status::{CreateProjectStatus, ProjectStatus, UpdateProjectStatus},
    project_tag::{CreateProjectTag, ProjectTag, UpdateProjectTag},
    repo::Repo,
};
use deployment::Deployment;
use futures_util::{StreamExt, TryStreamExt, stream::BoxStream};
use serde::Deserialize;
use ts_rs::TS;
use utils::{log_msg::LogMsg, response::ApiResponse};
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    middleware::ws::{AppWebSocket, WsUpgrade},
};

/// Forwards a JSON-Patch stream to a WebSocket until either side closes.
pub(crate) async fn forward_patch_stream(
    mut socket: AppWebSocket,
    stream: BoxStream<'static, Result<LogMsg, std::io::Error>>,
) {
    let mut stream = stream.map_ok(|msg| msg.to_ws_message_unchecked());
    loop {
        tokio::select! {
            item = stream.next() => match item {
                Some(Ok(msg)) => {
                    if socket.send(msg).await.is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    tracing::error!("stream error: {}", e);
                    break;
                }
                None => break,
            },
            msg = socket.recv() => match msg {
                Ok(Some(Message::Close(_))) | Ok(None) | Err(_) => break,
                Ok(Some(_)) => {}
            },
        }
    }
}

async fn load_project(deployment: &DeploymentImpl, id: Uuid) -> Result<Project, ApiError> {
    Project::find_by_id(&deployment.db().pool, id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))
}

fn require_name(name: &str, what: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::BadRequest(format!("{what} name is required")));
    }
    Ok(())
}

// --- Projects ---------------------------------------------------------------

async fn list_projects(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Project>>>, ApiError> {
    let projects = Project::find_all(&deployment.db().pool).await?;
    Ok(ResponseJson(ApiResponse::success(projects)))
}

async fn create_project(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateProject>,
) -> Result<ResponseJson<ApiResponse<Project>>, ApiError> {
    require_name(&payload.name, "Project")?;
    let project = Project::create(&deployment.db().pool, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(project)))
}

async fn get_project(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Project>>, ApiError> {
    let project = load_project(&deployment, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(project)))
}

async fn update_project(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<UpdateProject>,
) -> Result<ResponseJson<ApiResponse<Project>>, ApiError> {
    if let Some(pattern) = payload.external_id_pattern.clone().flatten()
        && let Err(e) = regex::Regex::new(&pattern)
    {
        return Err(ApiError::BadRequest(format!(
            "Invalid External ID pattern: {e}"
        )));
    }
    let project = Project::update(&deployment.db().pool, project_id, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(project)))
}

async fn delete_project(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    if Project::delete(&deployment.db().pool, project_id).await? == 0 {
        return Err(ApiError::Database(sqlx::Error::RowNotFound));
    }
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn stream_projects_ws(
    ws: WsUpgrade,
    State(deployment): State<DeploymentImpl>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        match deployment.events().stream_projects_raw().await {
            Ok(stream) => forward_patch_stream(socket, stream).await,
            Err(e) => tracing::warn!("projects WS failed: {}", e),
        }
    })
}

async fn stream_project_board_ws(
    ws: WsUpgrade,
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        match deployment
            .events()
            .stream_project_board_raw(project_id)
            .await
        {
            Ok(stream) => forward_patch_stream(socket, stream).await,
            Err(e) => tracing::warn!("project board WS failed: {}", e),
        }
    })
}

// --- Project repos ----------------------------------------------------------

#[derive(Debug, Deserialize, TS)]
pub struct LinkProjectRepoRequest {
    pub repo_id: Uuid,
}

async fn list_project_repos(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Repo>>>, ApiError> {
    let repos = ProjectRepo::find_repos_for_project(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(repos)))
}

async fn link_project_repo(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<LinkProjectRepoRequest>,
) -> Result<ResponseJson<ApiResponse<Vec<Repo>>>, ApiError> {
    let pool = &deployment.db().pool;
    load_project(&deployment, project_id).await?;
    if Repo::find_by_id(pool, payload.repo_id).await?.is_none() {
        return Err(ApiError::BadRequest("Repository not found".to_string()));
    }
    ProjectRepo::link(pool, project_id, payload.repo_id).await?;
    let repos = ProjectRepo::find_repos_for_project(pool, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(repos)))
}

async fn unlink_project_repo(
    Path((project_id, repo_id)): Path<(Uuid, Uuid)>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Repo>>>, ApiError> {
    let pool = &deployment.db().pool;
    ProjectRepo::unlink(pool, project_id, repo_id).await?;
    let repos = ProjectRepo::find_repos_for_project(pool, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(repos)))
}

// --- Statuses ---------------------------------------------------------------

async fn create_status(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateProjectStatus>,
) -> Result<ResponseJson<ApiResponse<ProjectStatus>>, ApiError> {
    require_name(&payload.name, "Status")?;
    load_project(&deployment, project_id).await?;
    let status = ProjectStatus::create(&deployment.db().pool, project_id, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(status)))
}

async fn update_status(
    Path(status_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<UpdateProjectStatus>,
) -> Result<ResponseJson<ApiResponse<ProjectStatus>>, ApiError> {
    let status = ProjectStatus::update(&deployment.db().pool, status_id, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(status)))
}

#[derive(Debug, Deserialize, TS)]
pub struct DeleteStatusQuery {
    /// Status that receives the deleted status's tickets. Defaults to the
    /// first other status.
    #[serde(default)]
    pub move_to: Option<Uuid>,
}

async fn delete_status(
    Path(status_id): Path<Uuid>,
    Query(query): Query<DeleteStatusQuery>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let pool = &deployment.db().pool;
    let status = ProjectStatus::find_by_id(pool, status_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let others: Vec<ProjectStatus> = ProjectStatus::find_by_project(pool, status.project_id)
        .await?
        .into_iter()
        .filter(|s| s.id != status_id)
        .collect();
    let move_to = match query.move_to {
        Some(id) if others.iter().any(|s| s.id == id) => id,
        Some(_) => {
            return Err(ApiError::BadRequest(
                "Target status does not belong to this project".to_string(),
            ));
        }
        None => {
            others
                .first()
                .ok_or_else(|| {
                    ApiError::BadRequest("A project needs at least one status".to_string())
                })?
                .id
        }
    };
    ProjectStatus::delete(pool, status_id, move_to).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

// --- Tags -------------------------------------------------------------------

async fn create_tag(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateProjectTag>,
) -> Result<ResponseJson<ApiResponse<ProjectTag>>, ApiError> {
    require_name(&payload.name, "Tag")?;
    load_project(&deployment, project_id).await?;
    let tag = ProjectTag::create(&deployment.db().pool, project_id, &payload)
        .await
        .map_err(conflict_on_unique("A tag with this name already exists"))?;
    Ok(ResponseJson(ApiResponse::success(tag)))
}

async fn update_tag(
    Path(tag_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<UpdateProjectTag>,
) -> Result<ResponseJson<ApiResponse<ProjectTag>>, ApiError> {
    let tag = ProjectTag::update(&deployment.db().pool, tag_id, &payload)
        .await
        .map_err(conflict_on_unique("A tag with this name already exists"))?;
    Ok(ResponseJson(ApiResponse::success(tag)))
}

async fn delete_tag(
    Path(tag_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    ProjectTag::delete(&deployment.db().pool, tag_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

pub(crate) fn conflict_on_unique(message: &'static str) -> impl Fn(sqlx::Error) -> ApiError {
    move |err| match &err {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            ApiError::Conflict(message.to_string())
        }
        _ => ApiError::Database(err),
    }
}

// --- Issues (project-scoped) ------------------------------------------------

async fn list_issues(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Issue>>>, ApiError> {
    let issues = Issue::find_by_project(&deployment.db().pool, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(issues)))
}

async fn create_issue(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateIssue>,
) -> Result<ResponseJson<ApiResponse<Issue>>, ApiError> {
    if payload.title.trim().is_empty() {
        return Err(ApiError::BadRequest("Ticket title is required".to_string()));
    }
    load_project(&deployment, project_id).await?;
    let issue = Issue::create(&deployment.db().pool, project_id, &payload).await?;
    Ok(ResponseJson(ApiResponse::success(issue)))
}

#[derive(Debug, Deserialize, TS)]
pub struct ExternalIdQuery {
    pub external_id: String,
}

/// The ticket with this External ID, if any (D17).
async fn find_issue_by_external_id(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ExternalIdQuery>,
) -> Result<ResponseJson<ApiResponse<Option<Issue>>>, ApiError> {
    let issue =
        Issue::find_by_external_id(&deployment.db().pool, project_id, query.external_id.trim())
            .await?;
    Ok(ResponseJson(ApiResponse::success(issue)))
}

#[derive(Debug, Deserialize, TS)]
pub struct DetectExternalIdRequest {
    pub text: String,
}

/// The External ID the project's pattern finds in `text` (D16).
async fn detect_external_id(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<DetectExternalIdRequest>,
) -> Result<ResponseJson<ApiResponse<Option<String>>>, ApiError> {
    let project = load_project(&deployment, project_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        local_deployment::intake::detect_for_project(&project, &payload.text),
    )))
}

async fn bulk_update_issues(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<Vec<BulkUpdateIssueItem>>,
) -> Result<ResponseJson<ApiResponse<Vec<Issue>>>, ApiError> {
    let pool = &deployment.db().pool;
    let mut updated = Vec::with_capacity(payload.len());
    for item in &payload {
        match Issue::find_by_id(pool, item.id).await? {
            Some(issue) if issue.project_id == project_id => {}
            _ => return Err(ApiError::Database(sqlx::Error::RowNotFound)),
        }
        updated.push(Issue::update(pool, item.id, &item.changes).await?);
    }
    Ok(ResponseJson(ApiResponse::success(updated)))
}

pub fn router() -> Router<DeploymentImpl> {
    let project = Router::new()
        .route(
            "/",
            get(get_project).put(update_project).delete(delete_project),
        )
        .route("/stream/ws", get(stream_project_board_ws))
        .route("/repos", get(list_project_repos).post(link_project_repo))
        .route("/repos/{repo_id}", delete(unlink_project_repo))
        .route("/statuses", post(create_status))
        .route("/tags", post(create_tag))
        .route("/issues", get(list_issues).post(create_issue))
        .route("/issues/bulk", post(bulk_update_issues))
        .route("/issues/by-external-id", get(find_issue_by_external_id))
        .route("/detect-external-id", post(detect_external_id));

    Router::new()
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/stream/ws", get(stream_projects_ws))
        .nest("/projects/{project_id}", project)
        .route(
            "/project-statuses/{status_id}",
            put(update_status).delete(delete_status),
        )
        .route("/project-tags/{tag_id}", put(update_tag).delete(delete_tag))
}
