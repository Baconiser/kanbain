//! The orchestrator (Phase 5, D22): coding an approved plan, with Stop and
//! Retry per subtask and per ticket.

use axum::{
    Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use local_deployment::orchestrator::{OrchestrationView, OrchestratorError};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

impl From<OrchestratorError> for ApiError {
    fn from(err: OrchestratorError) -> Self {
        match err {
            OrchestratorError::Database(e) => ApiError::Database(e),
            OrchestratorError::NotFound => ApiError::Database(sqlx::Error::RowNotFound),
            OrchestratorError::Invalid(msg) => ApiError::BadRequest(msg),
        }
    }
}

type ViewResponse = Result<ResponseJson<ApiResponse<OrchestrationView>>, ApiError>;

async fn get_orchestration(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().view(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

/// Starts coding the approved plan (or retries it when it was started).
async fn start_orchestration(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().start(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn stop_orchestration(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().stop_ticket(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn retry_orchestration(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().retry_ticket(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn stop_subtask(
    Path(subtask_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().stop_subtask(subtask_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn retry_subtask(
    Path(subtask_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> ViewResponse {
    let view = deployment.orchestrator().retry_subtask(subtask_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/issues/{issue_id}/coding", get(get_orchestration))
        .route("/issues/{issue_id}/coding/start", post(start_orchestration))
        .route("/issues/{issue_id}/coding/stop", post(stop_orchestration))
        .route("/issues/{issue_id}/coding/retry", post(retry_orchestration))
        .route("/subtasks/{subtask_id}/stop", post(stop_subtask))
        .route("/subtasks/{subtask_id}/retry", post(retry_subtask))
}
