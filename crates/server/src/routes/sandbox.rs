//! Docker sandbox status, model credentials and run events (Phase 1).

use axum::{
    Extension, Router,
    extract::State,
    middleware::from_fn_with_state,
    response::Json as ResponseJson,
    routing::{get, put},
};
use db::models::{
    execution_process::ExecutionProcess, execution_process_event::ExecutionProcessEvent,
};
use deployment::Deployment;
use local_deployment::sandbox::SandboxStatus;
use serde::Deserialize;
use services::services::secrets::{ANTHROPIC_API_KEY, OPENAI_API_KEY};
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError, middleware::load_execution_process_middleware};

#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProvider {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
}

#[derive(Debug, Deserialize, TS)]
pub struct SetModelCredential {
    pub provider: CredentialProvider,
    /// The API key; `null` removes the stored key.
    pub api_key: Option<String>,
}

async fn get_status(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<SandboxStatus>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.sandbox().status().await,
    )))
}

async fn set_credential(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<SetModelCredential>,
) -> Result<ResponseJson<ApiResponse<SandboxStatus>>, ApiError> {
    let name = match payload.provider {
        CredentialProvider::Anthropic => ANTHROPIC_API_KEY,
        CredentialProvider::OpenAi => OPENAI_API_KEY,
    };
    let secrets = deployment.secrets();
    let result = match payload.api_key.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::BadRequest("API key is empty".into())),
        Some(key) => secrets.set(name, key).await,
        None => secrets.remove(name).await,
    };
    result.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(ResponseJson(ApiResponse::success(
        deployment.sandbox().status().await,
    )))
}

async fn get_events(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<ExecutionProcessEvent>>>, ApiError> {
    let events = ExecutionProcessEvent::find_by_execution_process(
        &deployment.db().pool,
        execution_process.id,
    )
    .await?;
    Ok(ResponseJson(ApiResponse::success(events)))
}

pub(super) fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    let events = Router::new()
        .route("/execution-processes/{id}/events", get(get_events))
        .layer(from_fn_with_state(
            deployment.clone(),
            load_execution_process_middleware,
        ));
    Router::new()
        .route("/sandbox/status", get(get_status))
        .route("/sandbox/credentials", put(set_credential))
        .merge(events)
}
