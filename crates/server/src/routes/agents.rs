//! Agent library API (Phase 2): agent definitions, style blocks, skills,
//! import and export.

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json as ResponseJson, Response},
    routing::{get, post},
};
use executors::adapters::{
    AgentDefinition, AgentRole, AgentRuntime, Capability, RuntimeModelOption, StyleBlock,
    definition::{DEFAULT_TEST_COMMANDS, validate_name},
    suggested_models,
};
use serde::{Deserialize, Serialize};
use services::services::agent_library::{
    AgentSummary, ImportSummary, MAX_IMPORT_BYTES, SkillFileInfo, SkillSummary, StyleSummary,
};
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError};

/// Choices the agent editor offers.
#[derive(Debug, Serialize, TS)]
pub struct AgentLibraryMeta {
    pub roles: Vec<AgentRole>,
    pub runtimes: Vec<AgentRuntime>,
    pub capabilities: Vec<Capability>,
    pub claude_code_models: Vec<RuntimeModelOption>,
    pub opencode_models: Vec<RuntimeModelOption>,
    pub default_test_commands: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
pub struct CreateSkill {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct SkillFileQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// Comma-separated agent names; everything when absent.
    pub agents: Option<String>,
}

async fn meta() -> ResponseJson<ApiResponse<AgentLibraryMeta>> {
    ResponseJson(ApiResponse::success(AgentLibraryMeta {
        roles: AgentRole::all(),
        runtimes: vec![AgentRuntime::ClaudeCode, AgentRuntime::Opencode],
        capabilities: Capability::all(),
        claude_code_models: suggested_models(AgentRuntime::ClaudeCode),
        opencode_models: suggested_models(AgentRuntime::Opencode),
        default_test_commands: DEFAULT_TEST_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }))
}

async fn list_agents(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<AgentSummary>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.agent_library().list_agents().await?,
    )))
}

async fn get_agent(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
) -> Result<ResponseJson<ApiResponse<AgentDefinition>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.agent_library().get_agent(&name).await?,
    )))
}

async fn create_agent(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(def): ResponseJson<AgentDefinition>,
) -> Result<ResponseJson<ApiResponse<AgentDefinition>>, ApiError> {
    deployment.agent_library().create_agent(&def).await?;
    Ok(ResponseJson(ApiResponse::success(def)))
}

async fn update_agent(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
    ResponseJson(def): ResponseJson<AgentDefinition>,
) -> Result<ResponseJson<ApiResponse<AgentDefinition>>, ApiError> {
    let library = deployment.agent_library();
    library.get_agent(&name).await?;
    library.save_agent(&def, Some(&name)).await?;
    Ok(ResponseJson(ApiResponse::success(def)))
}

async fn delete_agent(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment.agent_library().delete_agent(&name).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn list_styles(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<StyleSummary>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.agent_library().list_styles().await?,
    )))
}

async fn create_style(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(style): ResponseJson<StyleBlock>,
) -> Result<ResponseJson<ApiResponse<StyleBlock>>, ApiError> {
    let library = deployment.agent_library();
    if library.get_style(&style.name).await.is_ok() {
        return Err(ApiError::Conflict(format!("style {} exists", style.name)));
    }
    library.save_style(&style, None).await?;
    Ok(ResponseJson(ApiResponse::success(style)))
}

async fn update_style(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
    ResponseJson(style): ResponseJson<StyleBlock>,
) -> Result<ResponseJson<ApiResponse<StyleBlock>>, ApiError> {
    let library = deployment.agent_library();
    library.get_style(&name).await?;
    library.save_style(&style, Some(&name)).await?;
    Ok(ResponseJson(ApiResponse::success(style)))
}

async fn delete_style(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment.agent_library().delete_style(&name).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn list_skills(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<SkillSummary>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.agent_library().list_skills().await?,
    )))
}

/// Start a skill with a `SKILL.md` holding its name and description.
async fn create_skill(
    State(deployment): State<DeploymentImpl>,
    ResponseJson(payload): ResponseJson<CreateSkill>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    validate_name(&payload.name).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let library = deployment.agent_library();
    if library.skill_files(&payload.name).await.is_ok() {
        return Err(ApiError::Conflict(format!("skill {} exists", payload.name)));
    }
    let description = payload.description.trim().replace('\n', " ");
    if description.is_empty() {
        return Err(ApiError::BadRequest("a skill needs a description".into()));
    }
    let skill_md = format!(
        "---\nname: {}\ndescription: {}\n---\n\n# {}\n\n",
        payload.name,
        serde_json::to_string(&description).unwrap_or_default(),
        payload.name
    );
    library
        .write_skill_file(&payload.name, "SKILL.md", skill_md.as_bytes())
        .await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn delete_skill(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment.agent_library().delete_skill(&name).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn list_skill_files(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
) -> Result<ResponseJson<ApiResponse<Vec<SkillFileInfo>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        deployment.agent_library().skill_files(&name).await?,
    )))
}

async fn read_skill_file(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
    Query(query): Query<SkillFileQuery>,
) -> Result<Response, ApiError> {
    let bytes = deployment
        .agent_library()
        .read_skill_file(&name, &query.path)
        .await?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}

async fn write_skill_file(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
    Query(query): Query<SkillFileQuery>,
    body: axum::body::Bytes,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment
        .agent_library()
        .write_skill_file(&name, &query.path, &body)
        .await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

async fn delete_skill_file(
    State(deployment): State<DeploymentImpl>,
    Path(name): Path<String>,
    Query(query): Query<SkillFileQuery>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    deployment
        .agent_library()
        .delete_skill_file(&name, &query.path)
        .await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

/// Import uploaded files: one or more `.zip` archives, or files whose
/// names are their paths in a chosen folder.
async fn import(
    State(deployment): State<DeploymentImpl>,
    mut multipart: Multipart,
) -> Result<ResponseJson<ApiResponse<ImportSummary>>, ApiError> {
    let library = deployment.agent_library();
    let mut files = Vec::new();
    let mut summary = ImportSummary::default();
    while let Some(field) = multipart.next_field().await? {
        let name = field.file_name().unwrap_or("upload").to_string();
        let bytes = field.bytes().await?;
        if name.to_ascii_lowercase().ends_with(".zip") {
            let part = library.import_zip(&bytes, &name).await?;
            summary.agents.extend(part.agents);
            summary.styles.extend(part.styles);
            summary.skills.extend(part.skills);
            summary.replaced.extend(part.replaced);
            summary.skipped.extend(part.skipped);
        } else {
            files.push((name, bytes.to_vec()));
        }
    }
    if !files.is_empty() {
        let part = library.import_files(files, None).await?;
        summary.agents.extend(part.agents);
        summary.styles.extend(part.styles);
        summary.skills.extend(part.skills);
        summary.replaced.extend(part.replaced);
        summary.skipped.extend(part.skipped);
    }
    Ok(ResponseJson(ApiResponse::success(summary)))
}

async fn export(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let agents: Option<Vec<String>> = query.agents.map(|list| {
        list.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let file_name = match agents.as_deref() {
        Some([one]) => format!("kanbain-agent-{one}.zip"),
        _ => "kanbain-agents.zip".to_string(),
    };
    let bytes = deployment
        .agent_library()
        .export_zip(agents.as_deref())
        .await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

pub(super) fn router() -> Router<DeploymentImpl> {
    let library = Router::new()
        .route("/meta", get(meta))
        .route("/agents", get(list_agents).post(create_agent))
        .route(
            "/agents/{name}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/styles", get(list_styles).post(create_style))
        .route(
            "/styles/{name}",
            axum::routing::put(update_style).delete(delete_style),
        )
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/{name}", axum::routing::delete(delete_skill))
        .route("/skills/{name}/files", get(list_skill_files))
        .route(
            "/skills/{name}/file",
            get(read_skill_file)
                .put(write_skill_file)
                .delete(delete_skill_file)
                .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            "/import",
            post(import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES + 1024 * 1024)),
        )
        .route("/export", get(export));
    Router::new().nest("/agent-library", library)
}
