//! The search service: keeps embedding spaces in line with the settings
//! (E4, blue/green), runs the index worker (E16–E20) and answers hybrid
//! searches (E5, E21).

use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use db::models::{issue::Issue, project::Project};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Notify, RwLock};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    client::{ClientError, Embedder, HttpEmbedder, InputKind},
    config::{EmbedderConfig, EmbedderProvider, SearchConfig, is_remote_url},
    sources::{self, KIND_COMMENT, TEXT_KINDS},
    store::{ChunkRow, Filter, JobCounts, Purpose, SourceKey, SourceMeta, Space, Store},
};

/// Secret slots for embedder and reranker API keys.
pub const TEXT_KEY_SLOT: &str = "search_text_api_key";
pub const CODE_KEY_SLOT: &str = "search_code_api_key";
pub const RERANKER_KEY_SLOT: &str = "search_reranker_api_key";

/// Where API keys come from (the encrypted secret store).
#[async_trait]
pub trait ApiKeys: Send + Sync {
    async fn key(&self, slot: &str) -> Option<String>;
}

const JOB_ISSUE: &str = "issue";
const JOB_COMMENT_REMOVED: &str = "comment_removed";
/// Debounce for ticket edits (E18).
const DEBOUNCE_MS: i64 = 1500;
/// How often an unreachable embedder is probed again.
const PROBE_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    #[ts(optional)]
    pub project_id: Option<Uuid>,
    /// Search every project (D47).
    #[serde(default)]
    #[ts(optional)]
    pub all_projects: Option<bool>,
    /// Adds that ticket's overlay (D45).
    #[serde(default)]
    #[ts(optional)]
    pub issue_id: Option<Uuid>,
    /// `ticket`, `comment`, `attachment`, `prior_work`, `run`, … (E21).
    #[serde(default)]
    #[ts(optional)]
    pub kinds: Option<Vec<String>>,
    #[serde(default)]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// Rerank with the configured reranker (Phase 3e).
    #[serde(default)]
    #[ts(optional)]
    pub rerank: Option<bool>,
    /// Leave out hits of this ticket (related tickets, duplicates).
    #[serde(default)]
    #[ts(optional)]
    pub exclude_issue_id: Option<Uuid>,
}

/// One hit: a location and a capped excerpt, not the content (D43).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SearchHit {
    pub kind: String,
    pub score: f64,
    /// Rank in the vector and keyword candidate lists (1-based).
    pub vector_rank: Option<u32>,
    pub keyword_rank: Option<u32>,
    pub excerpt: String,
    pub project_id: String,
    pub issue_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub attachment_id: Option<Uuid>,
    pub execution_process_id: Option<Uuid>,
    pub repo_id: Option<Uuid>,
    pub path: Option<String>,
    #[ts(type = "number | null")]
    pub start_line: Option<i64>,
    #[ts(type = "number | null")]
    pub end_line: Option<i64>,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    /// Vectors took part (an embedder is configured and reachable).
    pub semantic: bool,
    pub reranked: bool,
    /// Why vectors were not used, when they weren't.
    pub notice: Option<String>,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// A ticket similar to another one, with its best hit.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RelatedIssue {
    pub issue_id: Uuid,
    pub score: f64,
    /// Kinds of the hits, e.g. `ticket`, `run`.
    pub kinds: Vec<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SpaceStatus {
    pub space: Space,
    #[ts(type = "number")]
    pub chunks: i64,
    #[ts(type = "number")]
    pub embedded: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct EmbeddingStatus {
    /// A text embedder is configured.
    pub enabled: bool,
    pub spaces: Vec<SpaceStatus>,
    pub jobs: JobCounts,
    #[ts(type = "number")]
    pub chunks: i64,
    #[ts(type = "number")]
    pub sources: i64,
    /// The configured text embedder could not be probed.
    pub embedder_error: Option<String>,
    pub text_remote: bool,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
pub struct ReindexRequest {
    /// Only this project's sources; all when absent.
    #[serde(default)]
    #[ts(optional)]
    pub project_id: Option<Uuid>,
    /// Drop the active text space's vectors and embed everything again.
    #[serde(default)]
    #[ts(optional)]
    pub rebuild_vectors: Option<bool>,
}

fn space_config(space: &Space) -> EmbedderConfig {
    EmbedderConfig {
        provider: if space.provider == "tei" {
            EmbedderProvider::Tei
        } else {
            EmbedderProvider::Openai
        },
        base_url: space.base_url.clone(),
        model: space.model.clone(),
        query_prefix: space.query_prefix.clone(),
        document_prefix: space.doc_prefix.clone(),
        ..Default::default()
    }
}

/// Reciprocal rank fusion of ranked candidate lists (E5).
pub fn rrf(lists: &[&[i64]], k: f64) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The chunk text without its header line, cut at `max` characters.
pub fn excerpt(text: &str, max: usize) -> String {
    let body = text
        .split_once("\n\n")
        .map(|(_, rest)| rest)
        .unwrap_or(text)
        .trim();
    // A ticket chunk repeats its title as a heading; the header has it.
    let body = match body.strip_prefix("# ") {
        Some(rest) => rest.split_once('\n').map(|(_, r)| r.trim()).unwrap_or(rest),
        None => body,
    };
    if body.chars().count() <= max {
        return body.to_string();
    }
    let cut: String = body.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

struct Probe {
    hash: Option<String>,
    error: Option<String>,
    at: Option<Instant>,
}

#[derive(Clone)]
pub struct SearchService {
    store: Store,
    main: SqlitePool,
    config: Arc<RwLock<SearchConfig>>,
    keys: Arc<dyn ApiKeys>,
    wake: Arc<Notify>,
    probe: Arc<Mutex<Probe>>,
}

impl SearchService {
    pub async fn open(
        path: &Path,
        main: SqlitePool,
        config: Arc<RwLock<SearchConfig>>,
        keys: Arc<dyn ApiKeys>,
    ) -> Result<Self, sqlx::Error> {
        let store = Store::open(path).await?;
        store.requeue_running().await?;
        Ok(Self {
            store,
            main,
            config,
            keys,
            wake: Arc::default(),
            probe: Arc::new(Mutex::new(Probe {
                hash: None,
                error: None,
                at: None,
            })),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Starts the index worker and queues every ticket once, so the index
    /// catches up with changes made while the app was closed.
    pub fn start(&self) {
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(e) = service.enqueue_all(None).await {
                tracing::warn!("Queueing tickets for search failed: {e}");
            }
            service.worker().await;
        });
    }

    /// New search settings (Settings → Search).
    pub async fn set_config(&self, config: SearchConfig) {
        let changed = *self.config.read().await != config;
        *self.config.write().await = config;
        if changed {
            self.config_changed().await;
        }
    }

    /// The search settings changed.
    pub async fn config_changed(&self) {
        let mut probe = self.probe.lock().await;
        probe.at = None;
        probe.hash = None;
        drop(probe);
        self.wake.notify_one();
    }

    pub async fn enqueue_issue(&self, issue_id: Uuid) {
        if let Err(e) = self
            .store
            .enqueue(JOB_ISSUE, &issue_id.to_string(), DEBOUNCE_MS)
            .await
        {
            tracing::warn!("Queueing ticket {issue_id} for search failed: {e}");
        }
        self.wake.notify_one();
    }

    pub async fn comment_removed(&self, comment_id: Uuid) {
        if let Err(e) = self
            .store
            .enqueue(JOB_COMMENT_REMOVED, &comment_id.to_string(), 0)
            .await
        {
            tracing::warn!("Queueing a removed comment failed: {e}");
        }
        self.wake.notify_one();
    }

    async fn enqueue_all(&self, project_id: Option<Uuid>) -> Result<usize, sqlx::Error> {
        let ids: Vec<Uuid> = match project_id {
            Some(project) => {
                sqlx::query_scalar("SELECT id FROM issues WHERE project_id = ?")
                    .bind(project)
                    .fetch_all(&self.main)
                    .await?
            }
            None => {
                sqlx::query_scalar("SELECT id FROM issues")
                    .fetch_all(&self.main)
                    .await?
            }
        };
        for id in &ids {
            self.store.enqueue(JOB_ISSUE, &id.to_string(), 0).await?;
        }
        self.wake.notify_one();
        Ok(ids.len())
    }

    pub async fn reindex(&self, request: &ReindexRequest) -> Result<usize, sqlx::Error> {
        if request.rebuild_vectors.unwrap_or(false)
            && let Some(space) = self.store.active_space(Purpose::Text).await?
        {
            self.store.clear_space(&space).await?;
        }
        self.enqueue_all(request.project_id).await
    }

    async fn embedder_for(&self, config: &EmbedderConfig) -> HttpEmbedder {
        HttpEmbedder::new(config.clone(), self.keys.key(TEXT_KEY_SLOT).await)
    }

    // ---- worker ----

    async fn worker(&self) {
        loop {
            let mut busy = false;
            match self.run_jobs().await {
                Ok(n) => busy |= n > 0,
                Err(e) => tracing::warn!("Search indexing failed: {e}"),
            }
            if let Err(e) = self.sync_spaces().await {
                tracing::warn!("Updating embedding spaces failed: {e}");
            }
            match self.embed_pending().await {
                Ok(n) => busy |= n > 0,
                Err(e) => tracing::warn!("Embedding failed: {e}"),
            }
            if !busy {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        }
    }

    /// Re-chunks the queued tickets.
    async fn run_jobs(&self) -> Result<usize, sqlx::Error> {
        let jobs = self.store.take_due(20).await?;
        for job in &jobs {
            let result = match job.source_kind.as_str() {
                JOB_ISSUE => match Uuid::parse_str(&job.source_ref) {
                    Ok(id) => self.index_issue(id).await,
                    Err(_) => Ok(()),
                },
                JOB_COMMENT_REMOVED => self
                    .store
                    .delete_source(&SourceKey::text(KIND_COMMENT, &job.source_ref))
                    .await
                    .map(|_| ()),
                _ => Ok(()),
            };
            match result {
                Ok(()) => self.store.finish_job(job).await?,
                Err(e) => self.store.fail_job(job, &e.to_string()).await?,
            }
        }
        Ok(jobs.len())
    }

    /// Stores every text source of a ticket and removes those it no longer
    /// has. A deleted ticket loses everything.
    pub async fn index_issue(&self, issue_id: Uuid) -> Result<(), sqlx::Error> {
        let issue_ref = issue_id.to_string();
        let Some(issue) = Issue::find_by_id(&self.main, issue_id).await? else {
            self.store.delete_issue_sources(&issue_ref).await?;
            return Ok(());
        };
        let Some(project) = Project::find_by_id(&self.main, issue.project_id).await? else {
            return Ok(());
        };
        let found = sources::ticket_sources(&self.main, &issue, &project).await?;
        let meta = SourceMeta {
            project_id: project.id.to_string(),
            issue_id: issue_ref.clone(),
            blob_sha: None,
        };
        for source in &found {
            self.store
                .put_source(
                    &SourceKey::text(source.kind, &source.ref_id),
                    &meta,
                    &source.content_hash,
                    &source.chunks,
                )
                .await?;
        }
        for kind in TEXT_KINDS {
            for old in self.store.source_refs(kind, &issue_ref).await? {
                if !found.iter().any(|s| s.kind == *kind && s.ref_id == old) {
                    self.store
                        .delete_source(&SourceKey::text(kind, &old))
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Creates the building space for the configured text embedder, and
    /// activates building spaces that are complete.
    async fn sync_spaces(&self) -> Result<(), sqlx::Error> {
        let config = self.config.read().await.text.clone();
        if let Some(config) = config.filter(|c| !c.base_url.trim().is_empty()) {
            let mut probe = self.probe.lock().await;
            let due = probe.hash.is_none() && probe.at.is_none_or(|t| t.elapsed() > PROBE_RETRY);
            if due {
                probe.at = Some(Instant::now());
                let embedder = self.embedder_for(&config).await;
                match embedder.probe().await {
                    Ok(dimension) => {
                        probe.error = None;
                        let hash = crate::store::config_hash(Purpose::Text, &config, dimension);
                        if self
                            .store
                            .space_by_hash(&hash)
                            .await?
                            .is_none_or(|s| s.state == "retired")
                        {
                            let space = self
                                .store
                                .create_space(Purpose::Text, &config, dimension)
                                .await?;
                            tracing::info!(
                                "Building embedding space {} for {} ({dimension} dimensions)",
                                space.id,
                                config.model
                            );
                        }
                        // Building spaces of earlier settings are superseded.
                        for space in self.store.live_spaces().await? {
                            if space.purpose == "text"
                                && space.state == "building"
                                && space.config_hash != hash
                            {
                                self.store.retire(space.id).await?;
                            }
                        }
                        probe.hash = Some(hash);
                    }
                    Err(e) => {
                        tracing::warn!("Probing the text embedder failed: {e}");
                        probe.error = Some(e.to_string());
                    }
                }
            }
        }

        for space in self.store.live_spaces().await? {
            if space.state != "building" {
                continue;
            }
            let (total, embedded) = self.store.coverage(&space).await?;
            if embedded >= total && self.store.job_counts().await?.queued == 0 {
                self.store.activate(space.id).await?;
                tracing::info!("Embedding space {} is active ({total} chunks)", space.id);
            }
        }
        Ok(())
    }

    /// Embeds chunks without vectors, in every live space.
    async fn embed_pending(&self) -> Result<usize, sqlx::Error> {
        let batch = self
            .config
            .read()
            .await
            .text
            .as_ref()
            .map(|c| c.batch_size.max(1))
            .unwrap_or(32) as i64;
        let mut done = 0;
        for space in self.store.live_spaces().await? {
            if space.stale {
                continue;
            }
            let pending = self.store.pending_chunks(&space, batch).await?;
            if pending.is_empty() {
                continue;
            }
            let embedder = self.embedder_for(&space_config(&space)).await;
            let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
            let started = Instant::now();
            let result = embedder.embed(&texts, InputKind::Document).await;
            let latency = started.elapsed().as_millis() as u64;
            match result {
                Ok(out) => {
                    let _ = self
                        .store
                        .record_call(
                            Some(space.id),
                            Purpose::Text,
                            "document",
                            texts.len(),
                            out.tokens,
                            latency,
                            None,
                        )
                        .await;
                    if let Some(bad) = out
                        .vectors
                        .iter()
                        .find(|v| v.len() as i64 != space.dimension)
                    {
                        // E7: the server swapped the model behind the name.
                        let error = ClientError::Dimension {
                            expected: space.dimension as usize,
                            got: bad.len(),
                        }
                        .to_string();
                        self.store
                            .set_space_error(space.id, Some(&error), true)
                            .await?;
                        continue;
                    }
                    let pairs: Vec<_> = pending.into_iter().zip(out.vectors).collect();
                    done += self.store.store_vectors(&space, &pairs).await?;
                    if space.last_error.is_some() {
                        self.store.set_space_error(space.id, None, false).await?;
                    }
                }
                Err(e) => {
                    let _ = self
                        .store
                        .record_call(
                            Some(space.id),
                            Purpose::Text,
                            "document",
                            texts.len(),
                            None,
                            latency,
                            Some(&e.to_string()),
                        )
                        .await;
                    self.store
                        .set_space_error(space.id, Some(&e.to_string()), false)
                        .await?;
                    // Back off a little before the next attempt.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
        Ok(done)
    }

    // ---- search ----

    /// The active text space and an embedder for it, when vectors can be
    /// used.
    async fn query_space(&self) -> Option<Space> {
        self.config.read().await.text.as_ref()?;
        let space = self.store.active_space(Purpose::Text).await.ok()??;
        (!space.stale).then_some(space)
    }

    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResponse, sqlx::Error> {
        let started = Instant::now();
        let config = self.config.read().await.clone();
        let limit = request.limit.unwrap_or(config.limit).clamp(1, 200) as usize;
        let candidates = (config.candidates as usize).max(limit);
        let mut scopes = vec!["main".to_string()];
        if let Some(issue) = request.issue_id {
            scopes.push(issue.to_string());
        }
        let kinds = request.kinds.as_ref().map(|kinds| {
            kinds
                .iter()
                .filter(|k| TEXT_KINDS.contains(&k.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        });
        let filter = Filter {
            project_id: if request.all_projects.unwrap_or(false) {
                None
            } else {
                request.project_id.map(|p| p.to_string())
            },
            kinds,
            scopes,
        };
        // Leave room for the excluded ticket's own chunks.
        let fetch = candidates
            + if request.exclude_issue_id.is_some() {
                20
            } else {
                0
            };

        let keyword: Vec<i64> = self
            .store
            .keyword(&request.query, Purpose::Text, &filter, fetch)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        let mut notice = None;
        let mut vector: Vec<i64> = Vec::new();
        match self.query_space().await {
            Some(space) => {
                let embedder = self.embedder_for(&space_config(&space)).await;
                let t = Instant::now();
                match embedder
                    .embed(std::slice::from_ref(&request.query), InputKind::Query)
                    .await
                {
                    Ok(out) => {
                        let _ = self
                            .store
                            .record_call(
                                Some(space.id),
                                Purpose::Text,
                                "query",
                                1,
                                out.tokens,
                                t.elapsed().as_millis() as u64,
                                None,
                            )
                            .await;
                        match out.vectors.first() {
                            Some(v) if v.len() as i64 == space.dimension => {
                                vector = self
                                    .store
                                    .knn(&space, v, &filter, fetch)
                                    .await?
                                    .into_iter()
                                    .map(|(id, _)| id)
                                    .collect();
                            }
                            _ => {
                                notice = Some("The embedder's answer did not fit the index".into())
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Embedding a search query failed: {e}");
                        notice = Some(format!("Keyword search only: {e}"));
                    }
                }
            }
            None => {
                notice = Some(if config.text.is_some() {
                    "Keyword search only until the index is built".to_string()
                } else {
                    "Keyword search only: no embedder is configured".to_string()
                });
            }
        }
        let semantic = notice.is_none();

        let fused = rrf(&[&vector, &keyword], config.rrf_k as f64);
        let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        let rows: HashMap<i64, ChunkRow> = self
            .store
            .chunk_rows(&ids)
            .await?
            .into_iter()
            .map(|r| (r.chunk_id, r))
            .collect();
        let exclude = request.exclude_issue_id.map(|i| i.to_string());
        let rank = |list: &[i64], id: i64| list.iter().position(|x| *x == id).map(|p| p as u32 + 1);
        let hits: Vec<SearchHit> = fused
            .into_iter()
            .filter_map(|(id, score)| {
                let row = rows.get(&id)?;
                if exclude.as_deref() == Some(row.issue_id.as_str()) {
                    return None;
                }
                Some(hit_from_row(
                    row,
                    score,
                    rank(&vector, id),
                    rank(&keyword, id),
                    config.excerpt_chars as usize,
                ))
            })
            .take(limit)
            .collect();
        Ok(SearchResponse {
            hits,
            semantic,
            reranked: false,
            notice,
            took_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Tickets similar to `query`, best hit per ticket.
    pub async fn similar_issues(
        &self,
        query: &str,
        project_id: Uuid,
        exclude: Option<Uuid>,
        kinds: Option<Vec<String>>,
        limit: usize,
    ) -> Result<(Vec<RelatedIssue>, SearchResponse), sqlx::Error> {
        let candidates = self.config.read().await.candidates;
        let response = self
            .search(&SearchRequest {
                query: query.to_string(),
                project_id: Some(project_id),
                kinds,
                limit: Some(candidates),
                exclude_issue_id: exclude,
                ..Default::default()
            })
            .await?;
        let mut by_issue: Vec<RelatedIssue> = Vec::new();
        for hit in &response.hits {
            let Some(issue_id) = hit.issue_id else {
                continue;
            };
            // Hits come best first: a ticket ranks by its best chunk, so
            // long tickets with many chunks gain nothing.
            match by_issue.iter_mut().find(|r| r.issue_id == issue_id) {
                Some(related) => {
                    if !related.kinds.contains(&hit.kind) {
                        related.kinds.push(hit.kind.clone());
                    }
                }
                None => by_issue.push(RelatedIssue {
                    issue_id,
                    score: hit.score,
                    kinds: vec![hit.kind.clone()],
                    excerpt: hit.excerpt.clone(),
                }),
            }
        }
        by_issue.sort_by(|a, b| b.score.total_cmp(&a.score));
        by_issue.truncate(limit);
        Ok((by_issue, response))
    }

    /// Tickets related to a ticket, from its title and description (E21).
    pub async fn related(
        &self,
        issue: &Issue,
        limit: usize,
    ) -> Result<Vec<RelatedIssue>, sqlx::Error> {
        let mut query = issue.title.clone();
        if let Some(description) = &issue.description {
            query.push('\n');
            query.extend(description.chars().take(2000));
        }
        Ok(self
            .similar_issues(&query, issue.project_id, Some(issue.id), None, limit)
            .await?
            .0)
    }

    pub async fn status(&self) -> Result<EmbeddingStatus, sqlx::Error> {
        let config = self.config.read().await.clone();
        let mut spaces = Vec::new();
        for space in self.store.spaces().await? {
            let (chunks, embedded) = if space.state == "retired" {
                (0, 0)
            } else {
                self.store.coverage(&space).await?
            };
            spaces.push(SpaceStatus {
                space,
                chunks,
                embedded,
            });
        }
        let (chunks, sources) = self.store.chunk_count().await?;
        Ok(EmbeddingStatus {
            enabled: config.text.is_some(),
            spaces,
            jobs: self.store.job_counts().await?,
            chunks,
            sources,
            embedder_error: self.probe.lock().await.error.clone(),
            text_remote: config
                .text
                .as_ref()
                .is_some_and(|t| is_remote_url(&t.base_url)),
        })
    }
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

fn hit_from_row(
    row: &ChunkRow,
    score: f64,
    vector_rank: Option<u32>,
    keyword_rank: Option<u32>,
    excerpt_chars: usize,
) -> SearchHit {
    let issue_id = parse_uuid(&row.issue_id);
    let ref_tail = row.ref_id.rsplit(':').next().unwrap_or("");
    SearchHit {
        kind: row.kind.clone(),
        score,
        vector_rank,
        keyword_rank,
        excerpt: excerpt(&row.text, excerpt_chars),
        project_id: row.project_id.clone(),
        issue_id,
        comment_id: (row.kind == KIND_COMMENT)
            .then(|| parse_uuid(&row.ref_id))
            .flatten(),
        attachment_id: (row.kind == sources::KIND_ATTACHMENT)
            .then(|| parse_uuid(ref_tail))
            .flatten(),
        execution_process_id: (row.kind == sources::KIND_RUN)
            .then(|| parse_uuid(&row.ref_id))
            .flatten(),
        repo_id: parse_uuid(&row.repo_id),
        path: (!row.path.is_empty()).then(|| row.path.clone()),
        start_line: row.start_line,
        end_line: row.end_line,
        symbol: row.symbol.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_merges_lists() {
        let fused = rrf(&[&[1, 2, 3], &[3, 4]], 60.0);
        assert_eq!(fused[0].0, 3, "in both lists");
        assert_eq!(fused.len(), 4);
        assert!(fused[1].0 == 1);
    }

    #[test]
    fn excerpts_drop_the_header_and_cap() {
        assert_eq!(excerpt("KAN-1 · T\n\nBody text", 100), "Body text");
        assert_eq!(excerpt("H\n\nabcdef", 3), "abc…");
        assert_eq!(excerpt("H\n\n# Title\n\nBody", 100), "Body");
    }
}

#[cfg(test)]
mod service_tests {
    use db::models::{
        issue::{CreateIssue, UpdateIssue},
        issue_comment::{CreateIssueComment, IssueComment},
        project::CreateProject,
    };
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    use super::*;
    use crate::client::tests::{MockState, mock_server};

    struct NoKeys;
    #[async_trait]
    impl ApiKeys for NoKeys {
        async fn key(&self, _slot: &str) -> Option<String> {
            None
        }
    }

    async fn main_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                "sqlite::memory:"
                    .parse::<SqliteConnectOptions>()
                    .unwrap()
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        pool
    }

    fn embedder(base: &str, model: &str) -> EmbedderConfig {
        EmbedderConfig {
            provider: EmbedderProvider::Openai,
            base_url: format!("{base}/v1"),
            model: model.into(),
            ..Default::default()
        }
    }

    async fn issue(pool: &SqlitePool, project: Uuid, title: &str, description: &str) -> Issue {
        Issue::create(
            pool,
            project,
            &CreateIssue {
                title: title.into(),
                description: Some(description.into()),
                status_id: None,
                priority: None,
                sort_order: None,
                parent_issue_id: None,
                parent_issue_sort_order: None,
                external_id: None,
                mode: None,
                budget_usd: None,
            },
        )
        .await
        .unwrap()
    }

    /// Runs the worker steps until nothing is left to do.
    async fn settle(service: &SearchService) {
        for _ in 0..50 {
            sqlx::query("UPDATE index_jobs SET not_before = datetime('now', '-1 seconds')")
                .execute(service.store.pool())
                .await
                .unwrap();
            let jobs = service.run_jobs().await.unwrap();
            service.sync_spaces().await.unwrap();
            let embedded = service.embed_pending().await.unwrap();
            service.sync_spaces().await.unwrap();
            if jobs == 0 && embedded == 0 {
                return;
            }
        }
        panic!("indexing did not settle");
    }

    #[tokio::test]
    async fn indexes_searches_and_switches_models() {
        let state = MockState {
            dim: 64,
            ..Default::default()
        };
        let base = mock_server(state.clone()).await;
        let main = main_db().await;
        let project = Project::create(
            &main,
            &CreateProject {
                name: "Shop".into(),
                id_prefix: Some("KAN".into()),
                repo_ids: vec![],
            },
        )
        .await
        .unwrap();
        let reset = issue(
            &main,
            project.id,
            "Password reset email never arrives",
            "Users request a reset mail.",
        )
        .await;
        let other = issue(
            &main,
            project.id,
            "Dark mode for settings",
            "Add a dark theme toggle.",
        )
        .await;
        let mut ext = issue(
            &main,
            project.id,
            "Checkout total wrong",
            "Totals ignore discounts.",
        )
        .await;
        ext = Issue::update(
            &main,
            ext.id,
            &UpdateIssue {
                external_id: Some(Some("PROJ-123".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        IssueComment::create(
            &main,
            other.id,
            "user",
            &CreateIssueComment {
                message: "Use the system preference".into(),
                parent_id: None,
            },
        )
        .await
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(RwLock::new(SearchConfig {
            text: Some(embedder(&base, "model-a")),
            ..Default::default()
        }));
        let service = SearchService::open(
            &dir.path().join("embeddings.sqlite"),
            main.clone(),
            config.clone(),
            Arc::new(NoKeys),
        )
        .await
        .unwrap();
        service.enqueue_all(None).await.unwrap();
        settle(&service).await;

        let status = service.status().await.unwrap();
        let active: Vec<_> = status
            .spaces
            .iter()
            .filter(|s| s.space.state == "active")
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].chunks, active[0].embedded);
        assert_eq!(active[0].chunks, 4, "three tickets and a comment");

        let search = |query: &str| SearchRequest {
            query: query.into(),
            project_id: Some(project.id),
            ..Default::default()
        };
        let response = service
            .search(&search("reset password mail"))
            .await
            .unwrap();
        assert!(response.semantic, "{:?}", response.notice);
        assert_eq!(response.hits[0].issue_id, Some(reset.id));
        assert!(response.hits[0].vector_rank.is_some());
        // An id is found by keyword.
        let response = service.search(&search("PROJ-123")).await.unwrap();
        assert_eq!(response.hits[0].issue_id, Some(ext.id));
        assert_eq!(response.hits[0].keyword_rank, Some(1));
        // Comments are hits of their ticket.
        let response = service.search(&search("system preference")).await.unwrap();
        assert_eq!(response.hits[0].kind, "comment");
        assert_eq!(response.hits[0].issue_id, Some(other.id));

        // Related tickets leave out the ticket itself.
        let related = service.related(&reset, 5).await.unwrap();
        assert!(related.iter().all(|r| r.issue_id != reset.id));

        // Editing one ticket re-embeds only its changed chunk.
        let before = state
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        Issue::update(
            &main,
            other.id,
            &UpdateIssue {
                description: Some(Some("Add a dark theme toggle that follows the OS.".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        service.enqueue_issue(other.id).await;
        settle(&service).await;
        let after = state
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        assert_eq!(after - before, 1);

        // A new model builds a new space; search keeps using the active one.
        config.write().await.text = Some(embedder(&base, "model-b"));
        service.config_changed().await;
        service.sync_spaces().await.unwrap();
        let spaces = service.store.spaces().await.unwrap();
        assert_eq!(
            spaces
                .iter()
                .map(|s| (s.model.as_str(), s.state.as_str()))
                .collect::<Vec<_>>(),
            vec![("model-a", "active"), ("model-b", "building")]
        );
        let response = service
            .search(&search("reset password mail"))
            .await
            .unwrap();
        assert!(response.semantic);
        assert_eq!(response.hits[0].issue_id, Some(reset.id));
        settle(&service).await;
        let spaces = service.store.spaces().await.unwrap();
        assert_eq!(
            spaces
                .iter()
                .map(|s| (s.model.as_str(), s.state.as_str()))
                .collect::<Vec<_>>(),
            vec![("model-a", "retired"), ("model-b", "active")]
        );

        // A deleted ticket leaves the index.
        Issue::delete(&main, reset.id).await.unwrap();
        service.enqueue_issue(reset.id).await;
        settle(&service).await;
        let response = service
            .search(&search("reset password mail"))
            .await
            .unwrap();
        assert!(response.hits.iter().all(|h| h.issue_id != Some(reset.id)));

        // Without an embedder search falls back to keywords.
        config.write().await.text = None;
        let response = service.search(&search("discounts")).await.unwrap();
        assert!(!response.semantic);
        assert_eq!(response.hits[0].issue_id, Some(ext.id));
    }
}
