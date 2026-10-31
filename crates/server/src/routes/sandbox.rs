//! Docker sandbox status, model credentials and run events (Phase 1).

use axum::{
    Extension, Router,
    extract::{Path, State},
    middleware::from_fn_with_state,
    response::Json as ResponseJson,
    routing::{delete, get, post, put},
};
use db::models::{
    execution_process::ExecutionProcess, execution_process_event::ExecutionProcessEvent,
};
use deployment::Deployment;
use local_deployment::{
    claude_login::ClaudeLoginState, container::MAX_RUN_EXTENSION_MINUTES, sandbox::SandboxStatus,
};
use serde::{Deserialize, Serialize};
use services::services::{
    model_providers::{CustomModelProvider, api_key_secret},
    secrets::{ANTHROPIC_API_KEY, OPENAI_API_KEY},
};
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

/// A custom provider as the settings show it; the key never leaves the
/// backend.
#[derive(Debug, Serialize, TS)]
pub struct ModelProviderInfo {
    #[serde(flatten)]
    #[ts(flatten)]
    pub provider: CustomModelProvider,
    pub api_key_configured: bool,
}

#[derive(Debug, Deserialize, TS)]
pub struct SaveModelProvider {
    #[serde(flatten)]
    #[ts(flatten)]
    pub provider: CustomModelProvider,
    /// A new API key; `null` keeps the stored one.
    #[serde(default)]
    #[ts(optional)]
    pub api_key: Option<String>,
    /// Forget the stored key (for servers that take none).
    #[serde(default)]
    #[ts(optional)]
    pub clear_api_key: Option<bool>,
}

async fn provider_infos(deployment: &DeploymentImpl) -> Result<Vec<ModelProviderInfo>, ApiError> {
    let providers = deployment
        .model_providers()
        .list()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let mut infos = Vec::with_capacity(providers.len());
    for provider in providers {
        let api_key_configured = deployment
            .secrets()
            .contains(&api_key_secret(&provider.id))
            .await
            .unwrap_or(false);
        infos.push(ModelProviderInfo {
            provider,
            api_key_configured,
        });
    }
    Ok(infos)
}

async fn list_model_providers(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<ModelProviderInfo>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        provider_infos(&deployment).await?,
    )))
}

/// Add or update a custom provider (by id).
async fn save_model_provider(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<SaveModelProvider>,
) -> Result<ResponseJson<ApiResponse<Vec<ModelProviderInfo>>>, ApiError> {
    let provider = deployment
        .model_providers()
        .upsert(payload.provider)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let secret = api_key_secret(&provider.id);
    let secrets = deployment.secrets();
    let result = match payload.api_key.as_deref().map(str::trim) {
        Some(key) if !key.is_empty() => secrets.set(&secret, key).await,
        _ if payload.clear_api_key == Some(true) => secrets.remove(&secret).await,
        _ => Ok(()),
    };
    result.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(ResponseJson(ApiResponse::success(
        provider_infos(&deployment).await?,
    )))
}

async fn delete_model_provider(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<String>,
) -> Result<ResponseJson<ApiResponse<Vec<ModelProviderInfo>>>, ApiError> {
    deployment
        .model_providers()
        .remove(&id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    deployment
        .secrets()
        .remove(&api_key_secret(&id))
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(ResponseJson(ApiResponse::success(
        provider_infos(&deployment).await?,
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

/// A running run's time limit; `deadline` is `null` without one.
#[derive(Debug, Serialize, TS)]
pub struct RunTimeLimit {
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize, TS)]
pub struct ExtendRun {
    pub minutes: u32,
}

async fn get_time_limit(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> ResponseJson<ApiResponse<RunTimeLimit>> {
    ResponseJson(ApiResponse::success(RunTimeLimit {
        deadline: deployment.run_deadline(execution_process.id),
    }))
}

async fn extend_time_limit(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<ExtendRun>,
) -> Result<ResponseJson<ApiResponse<RunTimeLimit>>, ApiError> {
    if payload.minutes == 0 || payload.minutes > MAX_RUN_EXTENSION_MINUTES {
        return Err(ApiError::BadRequest(format!(
            "Extend by 1 to {MAX_RUN_EXTENSION_MINUTES} minutes"
        )));
    }
    let deadline = deployment
        .extend_run(execution_process.id, payload.minutes)
        .await
        .ok_or_else(|| ApiError::BadRequest("This run has no time limit to extend".into()))?;
    Ok(ResponseJson(ApiResponse::success(RunTimeLimit {
        deadline: Some(deadline),
    })))
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
        .route(
            "/execution-processes/{id}/time-limit",
            get(get_time_limit).post(extend_time_limit),
        )
        .layer(from_fn_with_state(
            deployment.clone(),
            load_execution_process_middleware,
        ));
    Router::new()
        .route("/sandbox/status", get(get_status))
        .route("/sandbox/credentials", put(set_credential))
        .route(
            "/sandbox/model-providers",
            get(list_model_providers).put(save_model_provider),
        )
        .route(
            "/sandbox/model-providers/{id}",
            delete(delete_model_provider),
        )
        .route(
            "/sandbox/claude-login",
            get(get_claude_login)
                .post(start_claude_login)
                .delete(cancel_claude_login),
        )
        .route("/sandbox/claude-login/code", post(submit_claude_login_code))
        .route("/sandbox/claude-account", delete(disconnect_claude_account))
        .merge(events)
}
