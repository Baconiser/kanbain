//! The planning flow and approval gate (Phase 4, D20, D21).

use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post, put},
};
use db::models::plan::{IssuePlanning, Plan, PlanDocument};
use deployment::Deployment;
use local_deployment::planning::{PlanTrigger, PlanningError, PlanningView};
use serde::Deserialize;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

impl From<PlanningError> for ApiError {
    fn from(err: PlanningError) -> Self {
        match err {
            PlanningError::Database(e) => ApiError::Database(e),
            PlanningError::NotFound => ApiError::Database(sqlx::Error::RowNotFound),
            PlanningError::AlreadyPlanning => ApiError::Conflict(err.to_string()),
            PlanningError::Invalid(msg) => ApiError::BadRequest(msg),
        }
    }
}

#[derive(Debug, Deserialize, TS)]
pub struct SendBackPlanRequest {
    pub feedback: String,
}

async fn get_planning(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<PlanningView>>, ApiError> {
    let view = deployment.planning().view(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(view)))
}

/// Assigns the ticket to the AI: starts a planner run.
async fn start_planning(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssuePlanning>>, ApiError> {
    let planning = deployment
        .planning()
        .start(issue_id, PlanTrigger::Assigned)
        .await?;
    Ok(ResponseJson(ApiResponse::success(planning)))
}

/// Saves an edited plan as a new version.
async fn edit_plan(
    Path(plan_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<PlanDocument>,
) -> Result<ResponseJson<ApiResponse<Plan>>, ApiError> {
    let plan = deployment.planning().edit(plan_id, payload).await?;
    Ok(ResponseJson(ApiResponse::success(plan)))
}

async fn approve_plan(
    Path(plan_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Plan>>, ApiError> {
    let plan = deployment.planning().approve(plan_id).await?;
    // No coding before approval, and coding right after it (D21, D22). A
    // failure to start shows on the timeline; the plan stays approved and
    // the ticket panel offers "Start coding".
    if let Err(e) = deployment.orchestrator().start(plan.issue_id).await {
        tracing::warn!(
            "Starting the coders for ticket {} failed: {e}",
            plan.issue_id
        );
        let _ = db::models::issue_event::IssueEvent::create(
            &deployment.db().pool,
            plan.issue_id,
            db::models::issue_event::IssueEventType::CodingFinished,
            "system",
            &serde_json::json!({"status": "not_started", "error": e.to_string()}),
            None,
        )
        .await;
    }
    Ok(ResponseJson(ApiResponse::success(plan)))
}

async fn send_back_plan(
    Path(plan_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<SendBackPlanRequest>,
) -> Result<ResponseJson<ApiResponse<Plan>>, ApiError> {
    let plan = deployment
        .planning()
        .send_back(plan_id, &payload.feedback)
        .await?;
    Ok(ResponseJson(ApiResponse::success(plan)))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/issues/{issue_id}/planning", get(get_planning))
        .route("/issues/{issue_id}/planning/start", post(start_planning))
        .route("/plans/{plan_id}", put(edit_plan))
        .route("/plans/{plan_id}/approve", post(approve_plan))
        .route("/plans/{plan_id}/send-back", post(send_back_plan))
}
