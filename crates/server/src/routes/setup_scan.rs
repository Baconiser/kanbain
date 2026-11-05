//! The setup agent's sandbox setup proposal for a project: start a scan,
//! review it, approve (with edits) or discard it. Plus the setup requests
//! agents made, which the next scan answers or the user dismisses.

use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::{
    agent_job::AgentJob,
    project::Project,
    project_setup_scan::{ProjectSetupScan, SetupProposal},
};
use deployment::Deployment;
use local_deployment::setup_scan::{SetupScanError, SetupScanView};
use serde::Serialize;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

impl From<SetupScanError> for ApiError {
    fn from(err: SetupScanError) -> Self {
        match err {
            SetupScanError::Database(e) => ApiError::Database(e),
            SetupScanError::NotFound => ApiError::Database(sqlx::Error::RowNotFound),
            SetupScanError::AlreadyScanning => ApiError::Conflict(err.to_string()),
            SetupScanError::Invalid(msg) => ApiError::BadRequest(msg),
        }
    }
}

/// The scan plus its job (cost, status), for the project settings.
#[derive(Debug, Serialize, TS)]
pub struct SetupScanResponse {
    #[serde(flatten)]
    #[ts(flatten)]
    pub view: SetupScanView,
    pub job: Option<AgentJob>,
}

async fn response(
    deployment: &DeploymentImpl,
    project_id: Uuid,
) -> Result<SetupScanResponse, ApiError> {
    let view = deployment.setup_scan().view(project_id).await?;
    let job = match view.scan.as_ref().and_then(|s| s.job_id) {
        Some(job_id) => AgentJob::find_by_id(&deployment.db().pool, job_id).await?,
        None => None,
    };
    Ok(SetupScanResponse { view, job })
}

async fn get_setup_scan(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<SetupScanResponse>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        response(&deployment, project_id).await?,
    )))
}

async fn start_setup_scan(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ProjectSetupScan>>, ApiError> {
    let scan = deployment.setup_scan().start(project_id).await?;
    Ok(ResponseJson(ApiResponse::success(scan)))
}

/// Applies the (possibly edited) proposal to the project settings.
async fn approve_setup_scan(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(proposal): Json<SetupProposal>,
) -> Result<ResponseJson<ApiResponse<Project>>, ApiError> {
    let project = deployment
        .setup_scan()
        .approve(project_id, &proposal)
        .await?;
    Ok(ResponseJson(ApiResponse::success(project)))
}

async fn dismiss_setup_request(
    Path((project_id, request_id)): Path<(Uuid, Uuid)>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment
        .setup_scan()
        .dismiss_request(project_id, request_id)
        .await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn discard_setup_scan(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment.setup_scan().discard(project_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/projects/{project_id}/setup-scan", get(get_setup_scan))
        .route(
            "/projects/{project_id}/setup-scan/start",
            post(start_setup_scan),
        )
        .route(
            "/projects/{project_id}/setup-scan/approve",
            post(approve_setup_scan),
        )
        .route(
            "/projects/{project_id}/setup-scan/discard",
            post(discard_setup_scan),
        )
        .route(
            "/projects/{project_id}/setup-requests/{request_id}/dismiss",
            post(dismiss_setup_request),
        )
}
