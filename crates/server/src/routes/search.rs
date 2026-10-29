//! Search over tickets and their history (Phase 3c, E21): hybrid search,
//! related tickets, possible duplicates at intake, index status and the
//! embedder settings' connection tests.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post, put},
};
use db::models::{
    issue::Issue,
    project::Project,
    project_status::{ProjectStatus, ProjectStatusKind},
    ticket_intake::TicketIntake,
};
use deployment::Deployment;
use embeddings::{
    client::{ConnectionTest, test_embedder, test_reranker},
    config::{EmbedderConfig, RerankerConfig},
    service::{
        CODE_KEY_SLOT, CodeLocation, EmbeddingStatus, RERANKER_KEY_SLOT, ReindexRequest,
        RelatedIssue, SearchHit, SearchRequest, TEXT_KEY_SLOT,
    },
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

const RELATED_LIMIT: usize = 5;

fn search_service(deployment: &DeploymentImpl) -> Result<&embeddings::SearchService, ApiError> {
    deployment
        .search()
        .ok_or_else(|| ApiError::Conflict("The search index is not available".to_string()))
}

/// A ticket as search results show it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct TicketRef {
    pub id: Uuid,
    pub project_id: Uuid,
    /// e.g. `KAN-12`
    pub internal_id: String,
    pub external_id: Option<String>,
    pub title: String,
    pub status: Option<String>,
    pub status_kind: Option<ProjectStatusKind>,
}

async fn ticket_refs(
    deployment: &DeploymentImpl,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, TicketRef>, ApiError> {
    let pool = &deployment.db().pool;
    let mut projects: HashMap<Uuid, Option<Project>> = HashMap::new();
    let mut refs = HashMap::new();
    for id in ids {
        if refs.contains_key(&id) {
            continue;
        }
        let Some(issue) = Issue::find_by_id(pool, id).await? else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = projects.entry(issue.project_id) {
            entry.insert(Project::find_by_id(pool, issue.project_id).await?);
        }
        let prefix = projects[&issue.project_id]
            .as_ref()
            .map(|p| p.id_prefix.clone())
            .unwrap_or_else(|| "KAN".into());
        let status = ProjectStatus::find_by_id(pool, issue.status_id).await?;
        refs.insert(
            id,
            TicketRef {
                id,
                project_id: issue.project_id,
                internal_id: issue.internal_id(&prefix),
                external_id: issue.external_id.clone(),
                title: issue.title.clone(),
                status: status.as_ref().map(|s| s.name.clone()),
                status_kind: status.map(|s| s.kind),
            },
        );
    }
    Ok(refs)
}

async fn repo_names(
    deployment: &DeploymentImpl,
    hits: &[SearchHit],
) -> Result<HashMap<Uuid, String>, ApiError> {
    let mut names = HashMap::new();
    for repo_id in hits.iter().filter_map(|h| h.repo_id) {
        if let std::collections::hash_map::Entry::Vacant(entry) = names.entry(repo_id)
            && let Some(repo) =
                db::models::repo::Repo::find_by_id(&deployment.db().pool, repo_id).await?
        {
            entry.insert(repo.name);
        }
    }
    Ok(names)
}

#[derive(Debug, Serialize, TS)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// The tickets the hits belong to, by id.
    pub tickets: HashMap<Uuid, TicketRef>,
    /// Names of the repos of code hits, by id.
    pub repos: HashMap<Uuid, String>,
    pub semantic: bool,
    pub notice: Option<String>,
    #[ts(type = "number")]
    pub took_ms: u64,
}

async fn search(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<SearchRequest>,
) -> Result<ResponseJson<ApiResponse<SearchResults>>, ApiError> {
    if request.query.trim().is_empty() {
        return Err(ApiError::BadRequest("The query is empty".to_string()));
    }
    let response = search_service(&deployment)?.search(&request).await?;
    let ids: Vec<Uuid> = response.hits.iter().filter_map(|h| h.issue_id).collect();
    let tickets = ticket_refs(&deployment, ids).await?;
    let repos = repo_names(&deployment, &response.hits).await?;
    Ok(ResponseJson(ApiResponse::success(SearchResults {
        hits: response.hits,
        tickets,
        repos,
        semantic: response.semantic,
        notice: response.notice,
        took_ms: response.took_ms,
    })))
}

/// A similar ticket with what makes it similar.
#[derive(Debug, Serialize, TS)]
pub struct RelatedTicket {
    pub ticket: TicketRef,
    pub score: f64,
    pub kinds: Vec<String>,
    pub excerpt: String,
    /// What its commits changed, e.g. `fn update in issue.rs` (E19).
    pub changed: Vec<String>,
}

/// A related code location, with the tickets whose commits changed it.
#[derive(Debug, Serialize, TS)]
pub struct RelatedCode {
    pub hit: SearchHit,
    pub repo_name: Option<String>,
    pub changed_by: Vec<TicketRef>,
}

/// Tickets and code related to a ticket (E21, E23).
#[derive(Debug, Serialize, TS)]
pub struct RelatedView {
    pub tickets: Vec<RelatedTicket>,
    pub code: Vec<RelatedCode>,
}

async fn with_code(
    deployment: &DeploymentImpl,
    code: Vec<CodeLocation>,
) -> Result<Vec<RelatedCode>, ApiError> {
    let ids: Vec<Uuid> = code.iter().flat_map(|c| c.changed_by.clone()).collect();
    let refs = ticket_refs(deployment, ids).await?;
    let hits: Vec<SearchHit> = code.iter().map(|c| c.hit.clone()).collect();
    let repos = repo_names(deployment, &hits).await?;
    Ok(code
        .into_iter()
        .map(|c| RelatedCode {
            repo_name: c.hit.repo_id.and_then(|r| repos.get(&r).cloned()),
            changed_by: c
                .changed_by
                .iter()
                .filter_map(|id| refs.get(id).cloned())
                .collect(),
            hit: c.hit,
        })
        .collect())
}

async fn with_tickets(
    deployment: &DeploymentImpl,
    related: Vec<RelatedIssue>,
) -> Result<Vec<RelatedTicket>, ApiError> {
    let ids: Vec<Uuid> = related.iter().map(|r| r.issue_id).collect();
    let refs = ticket_refs(deployment, ids).await?;
    Ok(related
        .into_iter()
        .filter_map(|r| {
            Some(RelatedTicket {
                ticket: refs.get(&r.issue_id)?.clone(),
                score: r.score,
                kinds: r.kinds,
                excerpt: r.excerpt,
                changed: r.changed,
            })
        })
        .collect())
}

async fn related(
    Path(issue_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<RelatedView>>, ApiError> {
    let issue = Issue::find_by_id(&deployment.db().pool, issue_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let related = search_service(&deployment)?
        .related(&issue, RELATED_LIMIT)
        .await?;
    Ok(ResponseJson(ApiResponse::success(RelatedView {
        tickets: with_tickets(&deployment, related.tickets).await?,
        code: with_code(&deployment, related.code).await?,
    })))
}

/// Tickets that may describe the same thing as a dropped one (E23).
async fn intake_duplicates(
    Path(intake_id): Path<Uuid>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<RelatedTicket>>>, ApiError> {
    let intake = TicketIntake::find_by_id(&deployment.db().pool, intake_id)
        .await?
        .ok_or(ApiError::Database(sqlx::Error::RowNotFound))?;
    let query = match &intake.enhanced {
        Some(enhanced) => format!("{}\n{}", enhanced.title, enhanced.description),
        None => intake.raw_text.chars().take(2000).collect(),
    };
    let (related, _) = search_service(&deployment)?
        .similar_issues(
            &query,
            intake.project_id,
            None,
            Some(vec!["ticket".to_string()]),
            RELATED_LIMIT,
        )
        .await?;
    Ok(ResponseJson(ApiResponse::success(
        with_tickets(&deployment, related).await?,
    )))
}

async fn status(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<EmbeddingStatus>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(
        search_service(&deployment)?.status().await?,
    )))
}

#[derive(Debug, Serialize, TS)]
pub struct ReindexResult {
    /// Tickets queued.
    pub queued: usize,
}

async fn reindex(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<ReindexRequest>,
) -> Result<ResponseJson<ApiResponse<ReindexResult>>, ApiError> {
    let queued = search_service(&deployment)?.reindex(&request).await?;
    Ok(ResponseJson(ApiResponse::success(ReindexResult { queued })))
}

/// Which embedder or reranker an API key belongs to.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SearchKeySlot {
    Text,
    Code,
    Reranker,
}

impl SearchKeySlot {
    fn secret(self) -> &'static str {
        match self {
            Self::Text => TEXT_KEY_SLOT,
            Self::Code => CODE_KEY_SLOT,
            Self::Reranker => RERANKER_KEY_SLOT,
        }
    }
}

#[derive(Debug, Deserialize, TS)]
pub struct TestEmbedderRequest {
    pub config: EmbedderConfig,
    pub slot: SearchKeySlot,
    /// A key typed but not saved yet; the stored one is used otherwise.
    #[serde(default)]
    #[ts(optional)]
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
pub struct TestRerankerRequest {
    pub config: RerankerConfig,
    #[serde(default)]
    #[ts(optional)]
    pub api_key: Option<String>,
}

async fn stored_key(deployment: &DeploymentImpl, slot: SearchKeySlot) -> Option<String> {
    deployment.secrets().get(slot.secret()).await.ok().flatten()
}

async fn test_embedder_route(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<TestEmbedderRequest>,
) -> ResponseJson<ApiResponse<ConnectionTest>> {
    let key = match request.api_key.filter(|k| !k.is_empty()) {
        Some(key) => Some(key),
        None => stored_key(&deployment, request.slot).await,
    };
    ResponseJson(ApiResponse::success(
        test_embedder(&request.config, key).await,
    ))
}

async fn test_reranker_route(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<TestRerankerRequest>,
) -> ResponseJson<ApiResponse<ConnectionTest>> {
    let key = match request.api_key.filter(|k| !k.is_empty()) {
        Some(key) => Some(key),
        None => stored_key(&deployment, SearchKeySlot::Reranker).await,
    };
    ResponseJson(ApiResponse::success(
        test_reranker(&request.config, key).await,
    ))
}

#[derive(Debug, Serialize, TS)]
pub struct SearchKeys {
    pub text: bool,
    pub code: bool,
    pub reranker: bool,
}

async fn get_keys(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<SearchKeys>>, ApiError> {
    let secrets = deployment.secrets();
    let has =
        |slot: SearchKeySlot| async move { secrets.contains(slot.secret()).await.unwrap_or(false) };
    Ok(ResponseJson(ApiResponse::success(SearchKeys {
        text: has(SearchKeySlot::Text).await,
        code: has(SearchKeySlot::Code).await,
        reranker: has(SearchKeySlot::Reranker).await,
    })))
}

#[derive(Debug, Deserialize, TS)]
pub struct SetSearchKey {
    pub slot: SearchKeySlot,
    /// `None` removes the key.
    #[serde(default)]
    #[ts(optional)]
    pub key: Option<String>,
}

/// Stores an embedder or reranker API key encrypted (like D33's).
async fn set_key(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<SetSearchKey>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let secrets = deployment.secrets();
    let result = match request.key.filter(|k| !k.trim().is_empty()) {
        Some(key) => secrets.set(request.slot.secret(), key.trim()).await,
        None => secrets.remove(request.slot.secret()).await,
    };
    result.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if let Some(search) = deployment.search() {
        search.config_changed().await;
    }
    Ok(ResponseJson(ApiResponse::success(())))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/search", post(search))
        .route("/issues/{issue_id}/related", get(related))
        .route("/intakes/{intake_id}/duplicates", get(intake_duplicates))
        .route("/embeddings/status", get(status))
        .route("/embeddings/reindex", post(reindex))
        .route("/search/test-embedder", post(test_embedder_route))
        .route("/search/test-reranker", post(test_reranker_route))
        .route("/search/keys", get(get_keys))
        .route("/search/keys", put(set_key))
}
