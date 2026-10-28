//! The git-history check (D18) and the files a ticket's commits changed
//! (E19).

use axum::{
    Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::{
    agent_job::AgentJob,
    issue_history::{IssueHistory, TicketCommit, TicketFileLink},
};
use deployment::Deployment;
use local_deployment::history::HistoryError;
use serde::Serialize;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

impl From<HistoryError> for ApiError {
    fn from(err: HistoryError) -> Self {
        match err {
            HistoryError::Database(e) => ApiError::Database(e),
            HistoryError::NotFound => ApiError::Database(sqlx::Error::RowNotFound),
        }
    }
}

#[derive(Debug, Serialize, TS)]
pub struct IssueHistoryView {
    /// `None` until the ticket was checked.
    pub history: Option<IssueHistory>,
    pub commits: Vec<TicketCommit>,
    pub files: Vec<TicketFileLink>,
    /// The latest analysis job.
    pub job: Option<AgentJob>,
    /// The project skips the permission prompt.
    pub always_allow: bool,
}

async fn history_view(
    deployment: &DeploymentImpl,
    issue_id: Uuid,
) -> Result<IssueHistoryView, ApiError> {
    let pool = &deployment.db().pool;
    let issue = db::models::issue::Issue::find_by_id(pool, issue_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let always_allow = db::models::project::Project::find_by_id(pool, issue.project_id)
        .await?
        .is_some_and(|p| p.settings.history_always_allow);
    let history = IssueHistory::find(pool, issue_id).await?;
    let job = match history.as_ref().and_then(|h| h.job_id) {
        Some(job_id) => AgentJob::find_by_id(pool, job_id).await?,
        None => None,
    };
    Ok(IssueHistoryView {
        history,
        commits: TicketCommit::find_by_issue(pool, issue_id).await?,
        files: TicketFileLink::find_by_issue(pool, issue_id).await?,
        job,
        always_allow,
    })
}

async fn get_history(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssueHistoryView>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        history_view(&deployment, issue_id).await?,
    )))
}

/// Searches the project's repos again.
async fn check_history(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssueHistoryView>>, ApiError> {
    deployment.history().check(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        history_view(&deployment, issue_id).await?,
    )))
}

/// "Allow analysis" for this ticket; starts the history agent.
async fn allow_history(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssueHistoryView>>, ApiError> {
    deployment.history().allow(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        history_view(&deployment, issue_id).await?,
    )))
}

async fn deny_history(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<IssueHistoryView>>, ApiError> {
    deployment.history().deny(issue_id).await?;
    Ok(ResponseJson(ApiResponse::success(
        history_view(&deployment, issue_id).await?,
    )))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/issues/{issue_id}/history", get(get_history))
        .route("/issues/{issue_id}/history/check", post(check_history))
        .route("/issues/{issue_id}/history/allow", post(allow_history))
        .route("/issues/{issue_id}/history/deny", post(deny_history))
}
