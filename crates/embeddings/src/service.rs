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
use db::models::{
    issue::Issue,
    issue_history::TicketFileChange,
    project::{Project, ProjectRepo},
    project_status::{ProjectStatus, ProjectStatusKind},
    repo::Repo,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Notify, RwLock};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    chunk,
    client::{ClientError, Embedder, HttpEmbedder, InputKind},
    config::{EmbedderConfig, EmbedderProvider, SearchConfig, is_remote_url},
    repo_index,
    sources::{self, KIND_COMMENT, TEXT_KINDS},
    store::{ChunkRow, Filter, JobCounts, KIND_CODE, Purpose, SourceKey, SourceMeta, Space, Store},
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
const JOB_REPO: &str = "repo";
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
    /// What the ticket's commits changed, e.g. `fn update in issue.rs`.
    pub changed: Vec<String>,
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
    /// Code would go to a remote embedder (E9).
    pub code_remote: bool,
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
    let lines: Vec<&str> = body.lines().collect();
    let body = if lines.len() > EXCERPT_LINES {
        format!("{}\n…", lines[..EXCERPT_LINES].join("\n"))
    } else {
        body.to_string()
    };
    if body.chars().count() <= max {
        return body;
    }
    let cut: String = body.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[derive(Default)]
struct Probe {
    hash: Option<String>,
    error: Option<String>,
    at: Option<Instant>,
}

/// A code location related to a ticket, with the tickets whose commits
/// changed it (E19).
#[derive(Debug, Clone, Serialize, TS)]
pub struct CodeLocation {
    pub hit: SearchHit,
    pub changed_by: Vec<Uuid>,
}

/// Similar tickets and code for one ticket (E21, E23).
#[derive(Debug, Clone, Serialize, TS)]
pub struct Related {
    pub tickets: Vec<RelatedIssue>,
    pub code: Vec<CodeLocation>,
}

/// Lines per code excerpt (§5).
const EXCERPT_LINES: usize = 20;

#[derive(Clone)]
pub struct SearchService {
    store: Store,
    main: SqlitePool,
    config: Arc<RwLock<SearchConfig>>,
    keys: Arc<dyn ApiKeys>,
    wake: Arc<Notify>,
    probes: Arc<Mutex<HashMap<&'static str, Probe>>>,
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
            probes: Arc::default(),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Starts the index worker and queues every ticket and project repo
    /// once, so the index catches up with changes made while the app was
    /// closed (cheap: unchanged content and blobs are skipped).
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

    /// The search settings or keys changed.
    pub async fn config_changed(&self) {
        self.probes.lock().await.clear();
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

    /// Queues a repo of a project for the code index (linked, merged into,
    /// or Reindex).
    pub async fn enqueue_repo(&self, project_id: Uuid, repo_id: Uuid) {
        if let Err(e) = self
            .store
            .enqueue(JOB_REPO, &format!("{project_id}:{repo_id}"), 0)
            .await
        {
            tracing::warn!("Queueing a repo for search failed: {e}");
        }
        self.wake.notify_one();
    }

    async fn enqueue_all(&self, project_id: Option<Uuid>) -> Result<usize, sqlx::Error> {
        let (issues, repos): (Vec<Uuid>, Vec<(Uuid, Uuid)>) = match project_id {
            Some(project) => (
                sqlx::query_scalar("SELECT id FROM issues WHERE project_id = ?")
                    .bind(project)
                    .fetch_all(&self.main)
                    .await?,
                sqlx::query_as(
                    "SELECT project_id, repo_id FROM project_repos WHERE project_id = ?",
                )
                .bind(project)
                .fetch_all(&self.main)
                .await?,
            ),
            None => (
                sqlx::query_scalar("SELECT id FROM issues")
                    .fetch_all(&self.main)
                    .await?,
                sqlx::query_as("SELECT project_id, repo_id FROM project_repos")
                    .fetch_all(&self.main)
                    .await?,
            ),
        };
        for (project, repo) in &repos {
            self.store
                .enqueue(JOB_REPO, &format!("{project}:{repo}"), 0)
                .await?;
        }
        for id in &issues {
            self.store.enqueue(JOB_ISSUE, &id.to_string(), 0).await?;
        }
        self.wake.notify_one();
        Ok(issues.len() + repos.len())
    }

    pub async fn reindex(&self, request: &ReindexRequest) -> Result<usize, sqlx::Error> {
        if request.rebuild_vectors.unwrap_or(false) {
            for purpose in [Purpose::Text, Purpose::Code] {
                if let Some(space) = self.store.active_space(purpose).await? {
                    self.store.clear_space(&space).await?;
                }
            }
        }
        self.enqueue_all(request.project_id).await
    }

    /// The embedder configured for a purpose: code falls back to the text
    /// embedder (E8).
    async fn configured(&self, purpose: Purpose) -> Option<EmbedderConfig> {
        let config = self.config.read().await;
        match purpose {
            Purpose::Text => config.text.clone(),
            Purpose::Code => config.code.clone().or_else(|| config.text.clone()),
        }
        .filter(|c| !c.base_url.trim().is_empty())
    }

    async fn key_for(&self, purpose: Purpose) -> Option<String> {
        let own_code = self.config.read().await.code.is_some();
        match purpose {
            Purpose::Code if own_code => self.keys.key(CODE_KEY_SLOT).await,
            _ => self.keys.key(TEXT_KEY_SLOT).await,
        }
    }

    async fn embedder_for(&self, purpose: Purpose, config: &EmbedderConfig) -> HttpEmbedder {
        HttpEmbedder::new(config.clone(), self.key_for(purpose).await)
    }

    /// Projects whose code may not go to this space's embedder (E9).
    async fn excluded_projects(&self, space: &Space) -> Result<Vec<String>, sqlx::Error> {
        if space.purpose != "code" || !is_remote_url(&space.base_url) {
            return Ok(vec![]);
        }
        Ok(Project::find_all(&self.main)
            .await?
            .into_iter()
            .filter(|p| !p.settings.allow_remote_code_embedding)
            .map(|p| p.id.to_string())
            .collect())
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

    /// Re-chunks the queued tickets and repos.
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
                JOB_REPO => {
                    let ids = job.source_ref.split_once(':').and_then(|(p, r)| {
                        Some((Uuid::parse_str(p).ok()?, Uuid::parse_str(r).ok()?))
                    });
                    match ids {
                        Some((project, repo)) => self.index_repo(project, repo).await,
                        None => Ok(()),
                    }
                }
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
    /// has, then refreshes its code overlay. A deleted ticket loses
    /// everything.
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
        self.index_overlay(&issue, &project).await
    }

    async fn project_repos(&self, project_id: Uuid) -> Result<Vec<Repo>, sqlx::Error> {
        ProjectRepo::find_repos_for_project(&self.main, project_id).await
    }

    /// Code at a repo's main branch (E17), incremental by blob SHA (E16).
    pub async fn index_repo(&self, project_id: Uuid, repo_id: Uuid) -> Result<(), sqlx::Error> {
        let project_ref = project_id.to_string();
        let repo_ref = repo_id.to_string();
        let linked = self
            .project_repos(project_id)
            .await?
            .into_iter()
            .find(|r| r.id == repo_id);
        let (Some(project), Some(repo)) =
            (Project::find_by_id(&self.main, project_id).await?, linked)
        else {
            // Unlinked or deleted: drop its code.
            for (path, _) in self
                .store
                .code_sources("main", &project_ref, &repo_ref)
                .await?
            {
                self.store
                    .delete_source(&code_key("main", &project_ref, &repo_ref, &path))
                    .await?;
            }
            return Ok(());
        };
        let excludes = project.settings.search_exclude_globs.clone();
        let path = repo.path.clone();
        let default_branch = repo.default_target_branch.clone();
        let listed = tokio::task::spawn_blocking(move || {
            let rev = repo_index::main_revision(&path, default_branch.as_deref())?;
            repo_index::list_files(&path, &rev)
        })
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        let files = match listed {
            Ok(files) => files,
            Err(e) => {
                tracing::warn!("Indexing {} failed: {e}", repo.name);
                return Ok(());
            }
        };
        let files: Vec<_> = files
            .into_iter()
            .filter(|f| chunk::code::indexable(&f.path, f.size, &excludes))
            .collect();
        let existing: HashMap<String, String> = self
            .store
            .code_sources("main", &project_ref, &repo_ref)
            .await?
            .into_iter()
            .collect();
        let changed: Vec<_> = files
            .iter()
            .filter(|f| {
                existing
                    .get(&f.path)
                    .is_none_or(|hash| *hash != chunk::code::content_hash(&f.blob_sha))
            })
            .cloned()
            .collect();
        self.store_files(&project, &repo, "main", "", &changed)
            .await?;
        let present: std::collections::HashSet<&str> =
            files.iter().map(|f| f.path.as_str()).collect();
        for path in existing.keys().filter(|p| !present.contains(p.as_str())) {
            self.store
                .delete_source(&code_key("main", &project_ref, &repo_ref, path))
                .await?;
        }
        if !changed.is_empty() {
            tracing::info!(
                "Indexed {} changed files of {} ({} files)",
                changed.len(),
                repo.name,
                files.len()
            );
        }
        Ok(())
    }

    /// Reads, chunks and stores files of a repo in a scope.
    async fn store_files(
        &self,
        project: &Project,
        repo: &Repo,
        scope: &str,
        issue_ref: &str,
        files: &[repo_index::RepoFile],
    ) -> Result<(), sqlx::Error> {
        let project_ref = project.id.to_string();
        let repo_ref = repo.id.to_string();
        for batch in files.chunks(200) {
            let path = repo.path.clone();
            let shas: Vec<String> = batch.iter().map(|f| f.blob_sha.clone()).collect();
            let blobs = tokio::task::spawn_blocking(move || repo_index::read_blobs(&path, &shas))
                .await
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
                .map_err(sqlx::Error::Protocol)?;
            for file in batch {
                let Some(text) = blobs
                    .get(&file.blob_sha)
                    .cloned()
                    .and_then(repo_index::as_text)
                else {
                    continue;
                };
                let chunks = chunk::code::chunk_code(
                    &format!("{}/{}", repo.name, file.path),
                    &file.path,
                    &text,
                    chunk::code::CODE_BUDGET,
                );
                self.store
                    .put_source(
                        &code_key(scope, &project_ref, &repo_ref, &file.path),
                        &SourceMeta {
                            project_id: project_ref.clone(),
                            issue_id: issue_ref.to_string(),
                            blob_sha: Some(file.blob_sha.clone()),
                        },
                        &chunk::code::content_hash(&file.blob_sha),
                        &chunks,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// The ticket overlay (D45): files the ticket's branches changed, at the
    /// branch, searched only for that ticket. Dropped once the ticket is
    /// done (its work is on main then).
    async fn index_overlay(&self, issue: &Issue, project: &Project) -> Result<(), sqlx::Error> {
        let scope = issue.id.to_string();
        let project_ref = project.id.to_string();
        let done = ProjectStatus::find_by_id(&self.main, issue.status_id)
            .await?
            .is_some_and(|s| s.kind == ProjectStatusKind::Done);
        let branches: Vec<String> = if done {
            vec![]
        } else {
            sqlx::query_scalar("SELECT branch FROM workspaces WHERE issue_id = ?")
                .bind(issue.id)
                .fetch_all(&self.main)
                .await?
        };
        let mut kept: Vec<(String, String)> = Vec::new();
        for repo in self.project_repos(project.id).await? {
            let repo_ref = repo.id.to_string();
            let path = repo.path.clone();
            let default_branch = repo.default_target_branch.clone();
            let branches = branches.clone();
            let changed = tokio::task::spawn_blocking(move || {
                let base = repo_index::main_revision(&path, default_branch.as_deref())?;
                let mut all = Vec::new();
                for branch in branches.iter().filter(|b| repo_index::has_branch(&path, b)) {
                    all.extend(repo_index::changed_files(&path, &base, branch)?);
                }
                Ok::<_, String>(all)
            })
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            let changed = match changed {
                Ok(changed) => changed,
                Err(e) => {
                    tracing::warn!("Ticket overlay for {} failed: {e}", repo.name);
                    continue;
                }
            };
            let existing: HashMap<String, String> = self
                .store
                .code_sources(&scope, &project_ref, &repo_ref)
                .await?
                .into_iter()
                .collect();
            let mut to_store = Vec::new();
            for (path, file) in changed {
                kept.push((repo_ref.clone(), path.clone()));
                match file {
                    Some(f)
                        if chunk::code::indexable(
                            &f.path,
                            f.size,
                            &project.settings.search_exclude_globs,
                        ) =>
                    {
                        if existing
                            .get(&path)
                            .is_none_or(|hash| *hash != chunk::code::content_hash(&f.blob_sha))
                        {
                            to_store.push(f);
                        }
                    }
                    // Deleted (or not indexable) on the branch: an empty
                    // source hides the main version for this ticket.
                    _ => {
                        self.store
                            .put_source(
                                &code_key(&scope, &project_ref, &repo_ref, &path),
                                &SourceMeta {
                                    project_id: project_ref.clone(),
                                    issue_id: scope.clone(),
                                    blob_sha: None,
                                },
                                "deleted",
                                &[],
                            )
                            .await?;
                    }
                }
            }
            self.store_files(project, &repo, &scope, &scope, &to_store)
                .await?;
        }
        for (repo_ref, path) in self.store.overlay_paths(&scope).await? {
            if !kept.iter().any(|(r, p)| *r == repo_ref && *p == path) {
                self.store
                    .delete_source(&code_key(&scope, &project_ref, &repo_ref, &path))
                    .await?;
            }
        }
        Ok(())
    }

    /// Creates building spaces for the configured embedders (text, and code
    /// with its own or the text embedder), and activates complete ones.
    async fn sync_spaces(&self) -> Result<(), sqlx::Error> {
        for purpose in [Purpose::Text, Purpose::Code] {
            let Some(config) = self.configured(purpose).await else {
                continue;
            };
            let mut probes = self.probes.lock().await;
            let probe = probes.entry(purpose.as_str()).or_default();
            let due = probe.hash.is_none() && probe.at.is_none_or(|t| t.elapsed() > PROBE_RETRY);
            if !due {
                continue;
            }
            probe.at = Some(Instant::now());
            let embedder = self.embedder_for(purpose, &config).await;
            match embedder.probe().await {
                Ok(dimension) => {
                    probe.error = None;
                    let hash = crate::store::config_hash(purpose, &config, dimension);
                    if self
                        .store
                        .space_by_hash(&hash)
                        .await?
                        .is_none_or(|s| s.state == "retired")
                    {
                        let space = self.store.create_space(purpose, &config, dimension).await?;
                        tracing::info!(
                            "Building {} embedding space {} for {} ({dimension} dimensions)",
                            purpose.as_str(),
                            space.id,
                            config.model
                        );
                    }
                    // Building spaces of earlier settings are superseded.
                    for space in self.store.live_spaces().await? {
                        if space.purpose == purpose.as_str()
                            && space.state == "building"
                            && space.config_hash != hash
                        {
                            self.store.retire(space.id).await?;
                        }
                    }
                    probe.hash = Some(hash);
                }
                Err(e) => {
                    tracing::warn!("Probing the {} embedder failed: {e}", purpose.as_str());
                    probe.error = Some(e.to_string());
                }
            }
        }

        for space in self.store.live_spaces().await? {
            if space.state != "building" {
                continue;
            }
            let exclude = self.excluded_projects(&space).await?;
            let (total, embedded) = self.store.coverage(&space, &exclude).await?;
            if embedded >= total && self.store.job_counts().await?.queued == 0 {
                self.store.activate(space.id).await?;
                tracing::info!("Embedding space {} is active ({total} chunks)", space.id);
            }
        }
        Ok(())
    }

    /// Embeds chunks without vectors, in every live space.
    async fn embed_pending(&self) -> Result<usize, sqlx::Error> {
        let mut done = 0;
        for space in self.store.live_spaces().await? {
            if space.stale {
                continue;
            }
            let purpose = Purpose::of_kind(if space.purpose == "code" {
                KIND_CODE
            } else {
                ""
            });
            let batch = self
                .configured(purpose)
                .await
                .map(|c| c.batch_size.max(1))
                .unwrap_or(32) as i64;
            let exclude = self.excluded_projects(&space).await?;
            let pending = self.store.pending_chunks(&space, batch, &exclude).await?;
            if pending.is_empty() {
                continue;
            }
            let embedder = self.embedder_for(purpose, &space_config(&space)).await;
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
                            purpose,
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
                            purpose,
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

    /// The active space of a purpose, when vectors can be used for it.
    async fn query_space(&self, purpose: Purpose) -> Option<Space> {
        self.configured(purpose).await?;
        let space = self.store.active_space(purpose).await.ok()??;
        (!space.stale).then_some(space)
    }

    /// Vector candidates of one purpose; `Err` says why there are none.
    async fn vector_candidates(
        &self,
        purpose: Purpose,
        query: &str,
        filter: &Filter,
        k: usize,
    ) -> Result<Result<Vec<i64>, String>, sqlx::Error> {
        let Some(space) = self.query_space(purpose).await else {
            return Ok(Err(if self.configured(purpose).await.is_some() {
                "Keyword search only until the index is built".to_string()
            } else {
                "Keyword search only: no embedder is configured".to_string()
            }));
        };
        let embedder = self.embedder_for(purpose, &space_config(&space)).await;
        let started = Instant::now();
        match embedder.embed(&[query.to_string()], InputKind::Query).await {
            Ok(out) => {
                let _ = self
                    .store
                    .record_call(
                        Some(space.id),
                        purpose,
                        "query",
                        1,
                        out.tokens,
                        started.elapsed().as_millis() as u64,
                        None,
                    )
                    .await;
                match out.vectors.first() {
                    Some(v) if v.len() as i64 == space.dimension => Ok(Ok(self
                        .store
                        .knn(&space, v, filter, k)
                        .await?
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect())),
                    _ => Ok(Err(
                        "The embedder's answer did not fit the index".to_string()
                    )),
                }
            }
            Err(e) => {
                tracing::warn!("Embedding a search query failed: {e}");
                Ok(Err(format!("Keyword search only: {e}")))
            }
        }
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
        let wanted = |kind: &str| {
            request
                .kinds
                .as_ref()
                .is_none_or(|kinds| kinds.iter().any(|k| k == kind))
        };
        let text_kinds: Vec<String> = TEXT_KINDS
            .iter()
            .filter(|k| wanted(k))
            .map(|k| k.to_string())
            .collect();
        let project_id = if request.all_projects.unwrap_or(false) {
            None
        } else {
            request.project_id.map(|p| p.to_string())
        };
        // Leave room for the excluded ticket's own chunks.
        let fetch = candidates
            + if request.exclude_issue_id.is_some() {
                20
            } else {
                0
            };

        let mut lists: Vec<Vec<i64>> = Vec::new();
        let mut vector_ids: Vec<i64> = Vec::new();
        let mut keyword_ids: Vec<i64> = Vec::new();
        let mut notice = None;
        let mut purposes = Vec::new();
        if !text_kinds.is_empty() {
            purposes.push((
                Purpose::Text,
                Filter {
                    project_id: project_id.clone(),
                    kinds: request.kinds.as_ref().map(|_| text_kinds.clone()),
                    scopes: vec!["main".to_string()],
                },
            ));
        }
        if wanted(KIND_CODE) {
            purposes.push((
                Purpose::Code,
                Filter {
                    project_id: project_id.clone(),
                    kinds: None,
                    scopes: scopes.clone(),
                },
            ));
        }
        for (purpose, filter) in &purposes {
            let keyword: Vec<i64> = self
                .store
                .keyword(&request.query, *purpose, filter, fetch)
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            match self
                .vector_candidates(*purpose, &request.query, filter, fetch)
                .await?
            {
                Ok(vector) => {
                    vector_ids.extend(&vector);
                    lists.push(vector);
                }
                Err(why) => {
                    notice.get_or_insert(why);
                }
            }
            keyword_ids.extend(&keyword);
            lists.push(keyword);
        }
        let semantic = notice.is_none();

        let refs: Vec<&[i64]> = lists.iter().map(Vec::as_slice).collect();
        let fused = rrf(&refs, config.rrf_k as f64);
        let ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        let rows: HashMap<i64, ChunkRow> = self
            .store
            .chunk_rows(&ids)
            .await?
            .into_iter()
            .map(|r| (r.chunk_id, r))
            .collect();
        // The ticket's overlay replaces the main version of its files.
        let overlay: Vec<(String, String)> = match request.issue_id {
            Some(issue) => self.store.overlay_paths(&issue.to_string()).await?,
            None => vec![],
        };
        let exclude = request.exclude_issue_id.map(|i| i.to_string());
        let rank = |list: &[i64], id: i64| list.iter().position(|x| *x == id).map(|p| p as u32 + 1);
        let ordered: Vec<(i64, f64)> = fused
            .into_iter()
            .filter(|(id, _)| {
                let Some(row) = rows.get(id) else {
                    return false;
                };
                if row.kind != KIND_CODE && exclude.as_deref() == Some(row.issue_id.as_str()) {
                    return false;
                }
                !(row.kind == KIND_CODE
                    && row.scope == "main"
                    && overlay
                        .iter()
                        .any(|(r, p)| *r == row.repo_id && *p == row.path))
            })
            .collect();

        let hits: Vec<SearchHit> = ordered
            .into_iter()
            .filter_map(|(id, score)| {
                let row = rows.get(&id)?;
                Some(hit_from_row(
                    row,
                    score,
                    rank(&vector_ids, id),
                    rank(&keyword_ids, id),
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
        let kinds =
            Some(kinds.unwrap_or_else(|| TEXT_KINDS.iter().map(|k| k.to_string()).collect()));
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
                    changed: vec![],
                }),
            }
        }
        by_issue.sort_by(|a, b| b.score.total_cmp(&a.score));
        by_issue.truncate(limit);
        Ok((by_issue, response))
    }

    /// Symbols a ticket's commits changed, as `path › symbol` (E19).
    pub async fn changed_symbols(
        &self,
        issue_id: Uuid,
        limit: usize,
    ) -> Result<Vec<String>, sqlx::Error> {
        let mut out: Vec<String> = Vec::new();
        for change in TicketFileChange::find_by_issue(&self.main, issue_id).await? {
            let symbols = self
                .store
                .symbols_at(&change.repo_id.to_string(), &change.path, &change.hunks.0)
                .await?;
            let file = change
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&change.path)
                .to_string();
            let entries: Vec<String> = if symbols.is_empty() {
                vec![file]
            } else {
                symbols
                    .into_iter()
                    .map(|s| format!("{} in {file}", s.rsplit(" › ").next().unwrap_or(&s)))
                    .collect()
            };
            for entry in entries {
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
            if out.len() >= limit {
                break;
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Tickets and code related to a ticket, from its title and
    /// description (E21, E23).
    pub async fn related(&self, issue: &Issue, limit: usize) -> Result<Related, sqlx::Error> {
        let mut query = issue.title.clone();
        if let Some(description) = &issue.description {
            query.push('\n');
            query.extend(description.chars().take(2000));
        }
        let (mut tickets, _) = self
            .similar_issues(&query, issue.project_id, Some(issue.id), None, limit)
            .await?;
        for ticket in &mut tickets {
            ticket.changed = self.changed_symbols(ticket.issue_id, 3).await?;
        }
        let code_hits = self
            .search(&SearchRequest {
                query,
                project_id: Some(issue.project_id),
                issue_id: Some(issue.id),
                kinds: Some(vec![KIND_CODE.to_string()]),
                limit: Some(limit as u32 * 2),
                ..Default::default()
            })
            .await?
            .hits;
        let mut code = Vec::new();
        for hit in code_hits {
            code.push(self.code_location(hit).await?);
        }
        Ok(Related { tickets, code })
    }

    /// A code hit with the tickets whose commits touched its lines.
    pub async fn code_location(&self, hit: SearchHit) -> Result<CodeLocation, sqlx::Error> {
        let mut changed_by = Vec::new();
        if let (Some(repo), Some(path), Some(start), Some(end)) = (
            hit.repo_id,
            hit.path.as_deref(),
            hit.start_line,
            hit.end_line,
        ) {
            for change in TicketFileChange::issues_for_path(&self.main, repo, path).await? {
                let touches = change.hunks.0.is_empty()
                    || change.hunks.0.iter().any(|(a, b)| *a <= end && *b >= start);
                if touches && !changed_by.contains(&change.issue_id) {
                    changed_by.push(change.issue_id);
                }
            }
        }
        Ok(CodeLocation { hit, changed_by })
    }

    pub async fn status(&self) -> Result<EmbeddingStatus, sqlx::Error> {
        let config = self.config.read().await.clone();
        let mut spaces = Vec::new();
        for space in self.store.spaces().await? {
            let (chunks, embedded) = if space.state == "retired" {
                (0, 0)
            } else {
                let exclude = self.excluded_projects(&space).await?;
                self.store.coverage(&space, &exclude).await?
            };
            spaces.push(SpaceStatus {
                space,
                chunks,
                embedded,
            });
        }
        let (chunks, sources) = self.store.chunk_count().await?;
        let probes = self.probes.lock().await;
        let embedder_error = ["text", "code"]
            .iter()
            .find_map(|p| probes.get(p).and_then(|probe| probe.error.clone()));
        Ok(EmbeddingStatus {
            enabled: config.text.is_some(),
            spaces,
            jobs: self.store.job_counts().await?,
            chunks,
            sources,
            embedder_error,
            text_remote: config
                .text
                .as_ref()
                .is_some_and(|t| is_remote_url(&t.base_url)),
            code_remote: config
                .code
                .as_ref()
                .or(config.text.as_ref())
                .is_some_and(|t| is_remote_url(&t.base_url)),
        })
    }
}

fn code_key(scope: &str, project_ref: &str, repo_ref: &str, path: &str) -> SourceKey {
    SourceKey {
        kind: KIND_CODE.to_string(),
        scope: scope.to_string(),
        ref_id: project_ref.to_string(),
        repo_id: repo_ref.to_string(),
        path: path.to_string(),
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
        issue_history::{NewCommitFile, NewTicketCommit, TicketCommit},
        project::CreateProject,
        repo::Repo,
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
            .filter(|s| s.space.state == "active" && s.space.purpose == "text")
            .collect();
        assert_eq!(active.len(), 1);
        // The code slot falls back to the text embedder in its own space.
        assert!(
            status
                .spaces
                .iter()
                .any(|s| s.space.purpose == "code" && s.space.state == "active")
        );
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
        let related = service.related(&reset, 5).await.unwrap().tickets;
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
                .filter(|s| s.purpose == "text")
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
                .filter(|s| s.purpose == "text")
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

    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "d@e.x")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "d@e.x")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }

    #[tokio::test]
    async fn indexes_code_incrementally_with_ticket_overlays() {
        let state = MockState {
            dim: 64,
            ..Default::default()
        };
        let base = mock_server(state.clone()).await;
        let main = main_db().await;
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("shop");
        std::fs::create_dir_all(repo_dir.join("src")).unwrap();
        let module = |extra: &str| {
            let mut s = String::new();
            for name in ["alpha", "beta", "gamma"] {
                s.push_str(&format!(
                    "pub fn {name}() -> u32 {{\n{}    0\n}}\n\n",
                    format!("    let {name}_value = 1;\n").repeat(60)
                ));
            }
            format!(
                "{s}pub fn assign_ticket_number(counter: u32) -> u32 {{\n    counter + 1{extra}\n}}\n"
            )
        };
        std::fs::write(repo_dir.join("src/issue.rs"), module("")).unwrap();
        std::fs::write(repo_dir.join("README.md"), "# Shop\n").unwrap();
        git(&repo_dir, &["init", "-q", "-b", "main"]);
        git(&repo_dir, &["add", "."]);
        git(&repo_dir, &["commit", "-qm", "init"]);

        let repo = Repo::find_or_create(&main, &repo_dir, "shop")
            .await
            .unwrap();
        let project = Project::create(
            &main,
            &CreateProject {
                name: "Shop".into(),
                id_prefix: Some("KAN".into()),
                repo_ids: vec![repo.id],
            },
        )
        .await
        .unwrap();
        let ticket = issue(
            &main,
            project.id,
            "Renumber tickets",
            "Numbers start at 100.",
        )
        .await;
        let other = issue(&main, project.id, "Unrelated", "Nothing.").await;

        let config = Arc::new(RwLock::new(SearchConfig {
            text: Some(embedder(&base, "model-a")),
            ..Default::default()
        }));
        let service = SearchService::open(
            &dir.path().join("embeddings.sqlite"),
            main.clone(),
            config,
            Arc::new(NoKeys),
        )
        .await
        .unwrap();
        service.enqueue_all(None).await.unwrap();
        settle(&service).await;

        let search = |issue_id: Option<Uuid>| SearchRequest {
            query: "assign ticket number counter".into(),
            project_id: Some(project.id),
            issue_id,
            kinds: Some(vec!["code".into()]),
            ..Default::default()
        };
        let hits = service.search(&search(None)).await.unwrap().hits;
        let hit = &hits[0];
        assert_eq!(hit.path.as_deref(), Some("src/issue.rs"));
        assert_eq!(hit.symbol.as_deref(), Some("fn assign_ticket_number"));
        assert_eq!(hit.repo_id, Some(repo.id));
        assert!(hit.vector_rank.is_some());

        // Changing one function re-embeds only that function.
        let before = state
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        std::fs::write(repo_dir.join("src/issue.rs"), module(" + 100")).unwrap();
        git(&repo_dir, &["commit", "-qam", "start at 100"]);
        service.enqueue_repo(project.id, repo.id).await;
        settle(&service).await;
        let after = state
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        assert_eq!(after - before, 1, "only the changed function");

        // An index from an older chunker (raw blob SHA as its hash) is
        // chunked again, though the blobs didn't change; chunks whose text
        // came out the same keep their vectors.
        sqlx::query("UPDATE sources SET content_hash = blob_sha WHERE kind = 'code'")
            .execute(service.store.pool())
            .await
            .unwrap();
        service.enqueue_repo(project.id, repo.id).await;
        settle(&service).await;
        let stale: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sources WHERE kind = 'code' AND content_hash NOT LIKE '%:c%'",
        )
        .fetch_one(service.store.pool())
        .await
        .unwrap();
        assert_eq!(stale, 0, "every file chunked by this version");
        let rechunked = state
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        assert_eq!(rechunked, after, "same chunks, no new embeddings");

        // E19: a ticket commit that touched assign_ticket_number names it,
        // and the code hit names the ticket; untouched functions don't.
        TicketCommit::replace_for_repo(
            &main,
            other.id,
            repo.id,
            &[NewTicketCommit {
                sha: "c0ffee".into(),
                author: "Dev".into(),
                committed_at: chrono::Utc::now(),
                subject: "KAN-2: start at 100".into(),
                branches: vec![],
                message_match: true,
                files: vec![NewCommitFile {
                    path: "src/issue.rs".into(),
                    additions: Some(1),
                    deletions: Some(1),
                    hunks: vec![(194, 194)],
                }],
            }],
        )
        .await
        .unwrap();
        assert_eq!(
            service.changed_symbols(other.id, 3).await.unwrap(),
            vec!["fn assign_ticket_number in issue.rs".to_string()]
        );
        let hits = service.search(&search(None)).await.unwrap().hits;
        let numbering = hits
            .iter()
            .find(|h| h.symbol.as_deref() == Some("fn assign_ticket_number"))
            .cloned()
            .unwrap();
        let alpha = hits
            .iter()
            .find(|h| h.symbol.as_deref() == Some("fn alpha"))
            .cloned()
            .unwrap_or_else(|| panic!("{hits:#?}"));
        assert_eq!(
            service.code_location(numbering).await.unwrap().changed_by,
            vec![other.id]
        );
        assert!(
            service
                .code_location(alpha)
                .await
                .unwrap()
                .changed_by
                .is_empty()
        );

        // A change on the ticket's branch is found by that ticket only.
        git(&repo_dir, &["checkout", "-qb", "kanbain/KAN-1-renumber"]);
        std::fs::write(
            repo_dir.join("src/numbering.rs"),
            "pub fn renumber_tickets_from_hundred() {}\n",
        )
        .unwrap();
        git(&repo_dir, &["add", "."]);
        git(&repo_dir, &["commit", "-qm", "KAN-1: renumber"]);
        git(&repo_dir, &["checkout", "-q", "main"]);
        sqlx::query(
            "INSERT INTO workspaces (id, branch, issue_id) VALUES (?, 'kanbain/KAN-1-renumber', ?)",
        )
        .bind(Uuid::new_v4())
        .bind(ticket.id)
        .execute(&main)
        .await
        .unwrap();
        service.enqueue_issue(ticket.id).await;
        settle(&service).await;
        let overlay_query = |issue_id: Option<Uuid>| SearchRequest {
            query: "renumber_tickets_from_hundred".into(),
            ..search(issue_id)
        };
        let mine = service
            .search(&overlay_query(Some(ticket.id)))
            .await
            .unwrap()
            .hits;
        assert_eq!(mine[0].path.as_deref(), Some("src/numbering.rs"));
        let theirs = service
            .search(&overlay_query(Some(other.id)))
            .await
            .unwrap()
            .hits;
        assert!(
            theirs
                .iter()
                .all(|h| h.path.as_deref() != Some("src/numbering.rs"))
        );
        let unscoped = service.search(&overlay_query(None)).await.unwrap().hits;
        assert!(
            unscoped
                .iter()
                .all(|h| h.path.as_deref() != Some("src/numbering.rs"))
        );
    }
}
