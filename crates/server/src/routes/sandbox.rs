//! Docker sandbox status, model credentials and run events (Phase 1).

use axum::{
    Extension, Router,
    extract::State,
    middleware::from_fn_with_state,
    response::Json as ResponseJson,
    routing::{get, post, put},
};
use db::models::{
    execution_process::ExecutionProcess, execution_process_event::ExecutionProcessEvent,
};
use deployment::Deployment;
use local_deployment::{claude_login::ClaudeLoginState, sandbox::SandboxStatus};
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

#[derive(Debug, Deserialize, TS)]
pub struct ClaudeLoginCode {
    pub code: String,
}

async fn get_claude_login(
    State(deployment): State<DeploymentImpl>,
) -> ResponseJson<ApiResponse<ClaudeLoginState>> {
    ResponseJson(ApiResponse::success(deployment.claude_login().state()))
}

/// Start "Connect Claude account": `claude setup-token` in a throwaway
/// container. Poll the state for the sign-in URL.
async fn start_claude_login(
    State(deployment): State<DeploymentImpl>,
) -> ResponseJson<ApiResponse<ClaudeLoginState>> {
    ResponseJson(ApiResponse::success(
        deployment.claude_login().start().await,
    ))
}

async fn submit_claude_login_code(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<ClaudeLoginCode>,
) -> Result<ResponseJson<ApiResponse<ClaudeLoginState>>, ApiError> {
    let state = deployment
        .claude_login()
        .submit_code(&payload.code)
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(ResponseJson(ApiResponse::success(state)))
}

async fn cancel_claude_login(
    State(deployment): State<DeploymentImpl>,
) -> ResponseJson<ApiResponse<ClaudeLoginState>> {
    deployment.claude_login().cancel();
    ResponseJson(ApiResponse::success(deployment.claude_login().state()))
}

/// Forget the connected Claude account; runs fall back to the API key.
async fn disconnect_claude_account(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<SandboxStatus>>, ApiError> {
    deployment
        .claude_login()
        .disconnect()
        .await
        .map_err(ApiError::BadRequest)?;
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
        .route(
            "/sandbox/claude-login",
            get(get_claude_login)
                .post(start_claude_login)
                .delete(cancel_claude_login),
        )
        .route("/sandbox/claude-login/code", post(submit_claude_login_code))
        .route(
            "/sandbox/claude-account",
            axum::routing::delete(disconnect_claude_account),
        )
        .merge(events)
}
