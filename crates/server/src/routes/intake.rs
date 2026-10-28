//! Ticket intake (D15–D17): drop a ticket, review the enhanced version,
//! create it or add a description version to the ticket with its External
//! ID.

use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::{
    agent_job::AgentJob, attachment_text::AttachmentText, file::File, issue::Issue,
    project::Project, ticket_intake::TicketIntake,
};
use deployment::Deployment;
use local_deployment::intake::{AcceptIntake, IntakeError};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use super::attachments::AttachmentResponse;
use crate::{DeploymentImpl, error::ApiError};

impl From<IntakeError> for ApiError {
    fn from(err: IntakeError) -> Self {
        match err {
            IntakeError::Database(e) => ApiError::Database(e),
            IntakeError::Issue(e) => e.into(),
            IntakeError::BadRequest(msg) => ApiError::BadRequest(msg),
            IntakeError::NotFound => ApiError::Database(sqlx::Error::RowNotFound),
        }
    }
}

#[derive(Debug, Deserialize, TS)]
pub struct StartIntake {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachment_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, TS)]
pub struct IntakeAttachment {
    pub attachment: AttachmentResponse,
    /// Extracted text; `None` for images.
    pub text: Option<AttachmentText>,
}

/// An intake with everything the review screen shows.
#[derive(Debug, Serialize, TS)]
pub struct IntakeView {
    pub intake: TicketIntake,
    pub job: Option<AgentJob>,
    pub attachments: Vec<IntakeAttachment>,
    /// The External ID to match: the agent's, else the detected one.
    pub external_id: Option<String>,
    /// The ticket that already has this External ID (D17).
    pub existing_issue: Option<Issue>,
    /// Its internal id, e.g. `KAN-12`.
    pub existing_internal_id: Option<String>,
}

async fn intake_view(deployment: &DeploymentImpl, id: Uuid) -> Result<IntakeView, ApiError> {
    let pool = &deployment.db().pool;
    let intake = TicketIntake::find_by_id(pool, id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let job = match intake.job_id {
        Some(job_id) => AgentJob::find_by_id(pool, job_id).await?,
        None => None,
    };
    let mut attachments = Vec::new();
    for attachment_id in TicketIntake::attachment_ids(pool, id).await? {
        if let Some(file) = File::find_by_id(pool, attachment_id).await? {
            attachments.push(IntakeAttachment {
                text: AttachmentText::find(pool, attachment_id).await?,
                attachment: AttachmentResponse::from_file(file),
            });
        }
    }
    let external_id = intake
        .enhanced
        .as_ref()
        .and_then(|e| e.external_id.clone())
        .or_else(|| intake.detected_external_id.clone());
    let existing_issue = match &external_id {
        Some(external_id) => {
            Issue::find_by_external_id(pool, intake.project_id, external_id).await?
        }
        None => None,
    };
    let existing_internal_id = match &existing_issue {
        Some(issue) => Project::find_by_id(pool, issue.project_id)
            .await?
            .map(|p| issue.internal_id(&p.id_prefix)),
        None => None,
    };
    Ok(IntakeView {
        intake,
        job,
        attachments,
        external_id,
        existing_issue,
        existing_internal_id,
    })
}

async fn start_intake(
    Path(project_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<StartIntake>,
) -> Result<ResponseJson<ApiResponse<IntakeView>>, ApiError> {
    let project = Project::find_by_id(&deployment.db().pool, project_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let intake = deployment
        .intake()
        .start(&project, &payload.text, &payload.attachment_ids)
        .await?;
    Ok(ResponseJson(ApiResponse::success(
        intake_view(&deployment, intake.id).await?,
    )))
}

async fn get_intake(
    Path(intake_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IntakeView>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        intake_view(&deployment, intake_id).await?,
    )))
}

async fn retry_intake(
    Path(intake_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IntakeView>>, ApiError> {
    deployment.intake().retry(intake_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        intake_view(&deployment, intake_id).await?,
    )))
}

async fn accept_intake(
    Path(intake_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<AcceptIntake>,
) -> Result<ResponseJson<ApiResponse<Issue>>, ApiError> {
    let issue = deployment.intake().accept(intake_id, &payload).await?;
    deployment.history().spawn_check(issue.id);
    Ok(ResponseJson(ApiResponse::success(issue)))
}

async fn discard_intake(
    Path(intake_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment.intake().discard(intake_id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/projects/{project_id}/intakes", post(start_intake))
        .route("/intakes/{intake_id}", get(get_intake))
        .route("/intakes/{intake_id}/retry", post(retry_intake))
        .route("/intakes/{intake_id}/accept", post(accept_intake))
        .route("/intakes/{intake_id}/discard", post(discard_intake))
}
