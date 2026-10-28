//! Sandboxed runs (Phase 1): one Docker container per workspace, reached by
//! `docker exec`, with the model proxy and egress proxy as the only way out.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use db::{
    DBService,
    models::{
        agent_job::AgentJob,
        execution_process::{ExecutionProcess, RunUsageDelta},
        execution_process_event::{ExecutionProcessEvent, ExecutionProcessEventType},
        issue::Issue,
        project::Project,
        repo::Repo,
        workspace::Workspace,
    },
};
use executors::{
    adapters::{AGENT_DIR, AgentLaunch, SKILLS_DIR, headless},
    env::{SANDBOX_PID_DIR, SandboxTarget},
    versions::{CLAUDE_CODE_PACKAGE, CLAUDE_CODE_VERSION, OPENCODE_PACKAGE, OPENCODE_VERSION},
};
use model_proxy::{
    ModelCallRecord, ModelProxy, PriceTable, Provider, ProxyEvents, StaticCredentials, Upstream,
};
use rand::{Rng, distr::Alphanumeric};
use sandbox::{
    AGENT_USER, CONTEXT_DIR, ContainerSpec, DockerSandbox, ExecOptions, FileEntry, KANBAIN_DIR,
    LABEL_EXECUTION, LABEL_INSTANCE, LABEL_KIND, LABEL_MANAGED, LABEL_WORKSPACE, Limits, OUT_DIR,
    Sandbox, WORKSPACE_DIR,
    docker::DockerInfo,
    egress::{self, Allowlist, EgressPolicy, EgressVerdict},
    image::{BaseImageArgs, base_image_tag, project_image_tag, stable_hash},
    network::{GATEWAY_HOST, SandboxNetwork},
    transfer::{self, PullOutcome},
};
use serde::Serialize;
use serde_json::json;
use services::services::{
    config::Config,
    secrets::{ANTHROPIC_API_KEY, CLAUDE_OAUTH_TOKEN, OPENAI_API_KEY, SecretStore},
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, mpsc};
use ts_rs::TS;
use uuid::Uuid;

const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com";
/// Largest `/kanbain/out` file stored on the run.
const MAX_HARVEST_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SandboxRunError {
    #[error("Docker is not available: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Sandbox(#[from] sandbox::SandboxError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// What the UI shows about the sandbox (setup screen, settings).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SandboxStatus {
    pub enabled: bool,
    pub docker_available: bool,
    pub docker_version: Option<String>,
    pub docker_desktop: bool,
    /// Why Docker can't be used, when it can't.
    pub error: Option<String>,
    pub image: Option<String>,
    pub image_ready: bool,
    pub image_building: bool,
    pub image_error: Option<String>,
    pub anthropic_key_configured: bool,
    /// A Claude account is connected; it is used instead of the API key.
    pub claude_account_connected: bool,
    pub openai_key_configured: bool,
    pub running_containers: usize,
}

struct Ready {
    docker: DockerSandbox,
    info: DockerInfo,
    network: SandboxNetwork,
    model_port: u16,
    egress_port: u16,
}

#[derive(Default, Clone)]
struct ImageState {
    base_tag: String,
    base_ready: bool,
    building: bool,
    error: Option<String>,
    gateway: Option<String>,
}

struct ActiveRun {
    container: String,
    egress_token: String,
    _permit: Option<OwnedSemaphorePermit>,
}

struct EgressGrant {
    run_id: Uuid,
    allowlist: Allowlist,
}

struct Inner {
    db: DBService,
    config: Arc<RwLock<Config>>,
    secrets: Arc<SecretStore>,
    instance: String,
    ready: Option<Ready>,
    error: Option<String>,
    /// Quick-access image state for status reads.
    image: std::sync::Mutex<ImageState>,
    /// Serializes image builds and gateway starts.
    build_lock: Mutex<()>,
    proxy: ModelProxy,
    egress_grants: Arc<std::sync::RwLock<HashMap<String, EgressGrant>>>,
    runs: Mutex<HashMap<Uuid, ActiveRun>>,
    slots: Arc<Semaphore>,
    /// Host port forwards of sandboxed dev servers, by execution.
    forwards: Mutex<HashMap<Uuid, Vec<tokio::task::JoinHandle<()>>>>,
}

#[derive(Clone)]
pub struct SandboxRuntime {
    inner: Arc<Inner>,
}

/// Hosts `claude setup-token` needs from inside the container. The browser
/// part of the login happens on the host.
pub const CLAUDE_LOGIN_HOSTS: &[&str] = &[
    "platform.claude.com",
    "claude.com",
    "claude.ai",
    "console.anthropic.com",
    "api.anthropic.com",
];

/// A throwaway login container and its environment.
pub struct LoginContainer {
    pub container: String,
    pub env: Vec<(String, String)>,
    egress_token: String,
}

/// File the task message of a headless job is written to, in
/// `/kanbain/context`.
pub const JOB_TASK_FILE: &str = "task.md";

/// One headless job to run (see [`SandboxRuntime::run_job`]).
pub struct JobRun<'a> {
    pub job_id: Uuid,
    pub launch: &'a AgentLaunch,
    /// Files for `/kanbain/context`, by relative path.
    pub context: Vec<(String, String)>,
    /// The first message.
    pub task: String,
    /// For a project's own sandbox image.
    pub project: Option<&'a Project>,
    pub budget_usd: Option<f64>,
    pub timeout: std::time::Duration,
}

/// One run to prepare in a workspace container.
pub struct PrepareRun<'a> {
    pub execution: &'a ExecutionProcess,
    pub workspace: &'a Workspace,
    pub repos: &'a [Repo],
    pub workspace_root: &'a Path,
    pub issue: Option<&'a Issue>,
    pub project: Option<&'a Project>,
    /// Agent runs take a concurrency slot and a model budget; scripts don't.
    pub agent_run: bool,
    /// Budget for this run in USD, already capped by the ticket's remainder.
    pub budget_usd: Option<f64>,
}

fn random_token(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        rand::rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(char::from)
            .collect::<String>()
    )
}

impl SandboxRuntime {
    /// Connect to Docker, clean up containers left by a previous launch,
    /// set up the networks and start both proxies. Never fails: when Docker
    /// is unusable the runtime reports why in [`SandboxStatus`]. Budget
    /// exhaustion is reported on the returned channel (execution ids).
    pub async fn new(
        db: DBService,
        config: Arc<RwLock<Config>>,
        secrets: Arc<SecretStore>,
    ) -> (Self, mpsc::UnboundedReceiver<Uuid>) {
        let instance = format!(
            "{:08x}",
            stable_hash(&[&utils::assets::asset_dir().to_string_lossy()])
        );
        let (budget_tx, budget_rx) = mpsc::unbounded_channel();
        let max_runs = config.read().await.sandbox.max_concurrent_runs.max(1) as usize;

        let prices = load_prices().await;
        let proxy = ModelProxy::new(
            prices,
            Arc::new(SecretCredentials {
                secrets: secrets.clone(),
            }),
            Arc::new(RunEvents {
                db: db.clone(),
                budget_tx,
            }),
        );
        let egress_grants = Arc::new(std::sync::RwLock::new(HashMap::new()));

        let (ready, error) = match Self::connect(&instance, &proxy, &db, &egress_grants).await {
            Ok(ready) => (Some(ready), None),
            Err(e) => {
                tracing::warn!("Sandbox unavailable: {e}");
                (None, Some(e.to_string()))
            }
        };
        let base_tag = base_image_tag(utils::version::APP_VERSION, &base_image_args());

        let runtime = Self {
            inner: Arc::new(Inner {
                db,
                config,
                secrets,
                instance,
                ready,
                error,
                image: std::sync::Mutex::new(ImageState {
                    base_tag,
                    ..Default::default()
                }),
                build_lock: Mutex::new(()),
                proxy,
                egress_grants,
                runs: Mutex::new(HashMap::new()),
                slots: Arc::new(Semaphore::new(max_runs)),
                forwards: Mutex::new(HashMap::new()),
            }),
        };

        // Build the image ahead of the first run; it takes minutes.
        if runtime.inner.ready.is_some() {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                if let Err(e) = runtime.ensure_image_and_gateway(&|_| {}).await {
                    tracing::error!("Preparing the sandbox image failed: {e}");
                }
            });
        }
        (runtime, budget_rx)
    }

    async fn connect(
        instance: &str,
        proxy: &ModelProxy,
        db: &DBService,
        egress_grants: &Arc<std::sync::RwLock<HashMap<String, EgressGrant>>>,
    ) -> Result<Ready, sandbox::SandboxError> {
        let (docker, info) = DockerSandbox::connect().await?;
        let removed = docker.remove_orphans(instance, &[]).await?;
        if removed > 0 {
            tracing::info!("Removed {removed} sandbox containers left from a previous run");
        }
        let network = docker
            .ensure_networks(instance, info.is_docker_desktop)
            .await?;

        let model_listener = tokio::net::TcpListener::bind((network.bind_ip, 0)).await?;
        let model_port = model_listener.local_addr()?.port();
        tokio::spawn(proxy.clone().serve(model_listener));

        let egress_listener = egress::bind(network.bind_ip).await?;
        let egress_port = egress_listener.local_addr()?.port();
        tokio::spawn(egress::serve(
            egress_listener,
            Arc::new(RunEgressPolicy {
                db: db.clone(),
                grants: egress_grants.clone(),
            }),
        ));
        tracing::info!(
            "Sandbox ready: Docker {} ({}), proxies on {}:{model_port} and {}:{egress_port}",
            info.version,
            info.operating_system,
            network.bind_ip,
            network.bind_ip
        );
        Ok(Ready {
            docker,
            info,
            network,
            model_port,
            egress_port,
        })
    }

    pub async fn enabled(&self) -> bool {
        self.inner.config.read().await.sandbox.enabled
    }

    pub fn docker(&self) -> Option<&DockerSandbox> {
        self.inner.ready.as_ref().map(|r| &r.docker)
    }

    fn ready(&self) -> Result<&Ready, SandboxRunError> {
        self.inner.ready.as_ref().ok_or_else(|| {
            SandboxRunError::Unavailable(
                self.inner
                    .error
                    .clone()
                    .unwrap_or_else(|| "not connected".into()),
            )
        })
    }

    pub async fn status(&self) -> SandboxStatus {
        let image = self.inner.image.lock().unwrap().clone();
        let running_containers = match &self.inner.ready {
            Some(ready) => ready
                .docker
                .list_by_labels(&[
                    (LABEL_INSTANCE, self.inner.instance.as_str()),
                    (LABEL_KIND, "agent"),
                ])
                .await
                .map(|ids| ids.len())
                .unwrap_or(0),
            None => 0,
        };
        SandboxStatus {
            enabled: self.enabled().await,
            docker_available: self.inner.ready.is_some(),
            docker_version: self.inner.ready.as_ref().map(|r| r.info.version.clone()),
            docker_desktop: self
                .inner
                .ready
                .as_ref()
                .is_some_and(|r| r.info.is_docker_desktop),
            error: self.inner.error.clone(),
            image: Some(image.base_tag.clone()),
            image_ready: image.base_ready,
            image_building: image.building,
            image_error: image.error.clone(),
            anthropic_key_configured: self
                .inner
                .secrets
                .contains(ANTHROPIC_API_KEY)
                .await
                .unwrap_or(false)
                || std::env::var("ANTHROPIC_API_KEY").is_ok(),
            claude_account_connected: self
                .inner
                .secrets
                .contains(CLAUDE_OAUTH_TOKEN)
                .await
                .unwrap_or(false),
            openai_key_configured: self
                .inner
                .secrets
                .contains(OPENAI_API_KEY)
                .await
                .unwrap_or(false)
                || std::env::var("OPENAI_API_KEY").is_ok(),
            running_containers,
        }
    }

    /// Build the base image if needed and (re)start the gateway sidecar.
    /// Concurrent callers wait for the same build.
    async fn ensure_image_and_gateway(
        &self,
        log: &(dyn Fn(String) + Send + Sync),
    ) -> Result<String, SandboxRunError> {
        let ready = self.ready()?;
        let _build = self.inner.build_lock.lock().await;
        let (tag, base_ready, gateway) = {
            let image = self.inner.image.lock().unwrap();
            (
                image.base_tag.clone(),
                image.base_ready,
                image.gateway.clone(),
            )
        };
        if !base_ready {
            {
                let mut image = self.inner.image.lock().unwrap();
                image.building = true;
                image.error = None;
            }
            if !ready.docker.image_exists(&tag).await? {
                log(format!(
                    "Building the sandbox image {tag}. The first build takes several minutes."
                ));
            }
            let result = ready
                .docker
                .ensure_base_image(&tag, &base_image_args(), |line| {
                    tracing::debug!("[sandbox image] {line}")
                })
                .await;
            let mut image = self.inner.image.lock().unwrap();
            image.building = false;
            if let Err(e) = result {
                image.error = Some(e.to_string());
                return Err(e.into());
            }
            image.base_ready = true;
        }
        let gateway_running = match &gateway {
            Some(id) => ready.docker.is_running(id).await.unwrap_or(false),
            None => false,
        };
        if !gateway_running {
            let id = ready
                .docker
                .start_gateway(&ready.network, &tag, ready.model_port, ready.egress_port)
                .await?;
            self.inner.image.lock().unwrap().gateway = Some(id);
        }
        Ok(tag)
    }

    /// Image for a project: its own Dockerfile when configured, else the base.
    async fn image_for(
        &self,
        project: Option<&Project>,
        base_tag: &str,
        log: &(dyn Fn(String) + Send + Sync),
    ) -> Result<String, SandboxRunError> {
        let Some((project, dockerfile)) = project.and_then(|p| {
            p.settings
                .sandbox_dockerfile
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .map(|d| (p, PathBuf::from(d)))
        }) else {
            return Ok(base_tag.to_string());
        };
        let contents = tokio::fs::read_to_string(&dockerfile).await.map_err(|e| {
            sandbox::SandboxError::Other(format!(
                "cannot read project Dockerfile {}: {e}",
                dockerfile.display()
            ))
        })?;
        let tag = project_image_tag(&project.id.simple().to_string()[..8], &contents, base_tag);
        let ready = self.ready()?;
        if !ready.docker.image_exists(&tag).await? {
            log(format!("Building the project image {tag}"));
        }
        let context_dir = dockerfile.parent().unwrap_or(Path::new("."));
        ready
            .docker
            .ensure_project_image(&tag, context_dir, base_tag, |line| {
                tracing::debug!("[project image] {line}")
            })
            .await?;
        Ok(tag)
    }

    /// The running container of a workspace, if any.
    pub async fn workspace_container(&self, workspace_id: Uuid) -> Option<String> {
        let ready = self.inner.ready.as_ref()?;
        let ids = ready
            .docker
            .list_by_labels(&[
                (LABEL_INSTANCE, self.inner.instance.as_str()),
                (LABEL_WORKSPACE, workspace_id.to_string().as_str()),
            ])
            .await
            .ok()?;
        for id in ids {
            if ready.docker.is_running(&id).await.unwrap_or(false) {
                return Some(id);
            }
            let _ = ready.docker.remove(&id).await;
        }
        None
    }

    async fn ensure_workspace_container(
        &self,
        workspace: &Workspace,
        image: &str,
    ) -> Result<String, SandboxRunError> {
        if let Some(id) = self.workspace_container(workspace.id).await {
            return Ok(id);
        }
        let ready = self.ready()?;
        let settings = self.inner.config.read().await.sandbox.clone();
        let spec = ContainerSpec {
            name: format!(
                "kanbain-ws-{}-{}",
                &workspace.id.simple().to_string()[..8],
                self.inner.instance
            ),
            image: image.to_string(),
            labels: HashMap::from([
                (LABEL_MANAGED.to_string(), "true".to_string()),
                (LABEL_INSTANCE.to_string(), self.inner.instance.clone()),
                (LABEL_KIND.to_string(), "agent".to_string()),
                (LABEL_WORKSPACE.to_string(), workspace.id.to_string()),
            ]),
            limits: Limits {
                cpus: settings.cpus,
                memory_mb: settings.memory_mb as u64,
                pids: settings.pids_limit as i64,
            },
            networks: vec![(ready.network.internal_network.clone(), vec![])],
            volumes: vec![KANBAIN_DIR.to_string()],
            working_dir: Some(WORKSPACE_DIR.to_string()),
            user: Some(AGENT_USER.to_string()),
            ..Default::default()
        };
        // A stale container with the same name (e.g. a stopped one) is replaced.
        ready.docker.remove(&spec.name).await?;
        Ok(ready.docker.create(&spec).await?)
    }

    /// Prepare a run: image, container, code and context in, tokens out.
    /// `log` receives progress lines for the run's log.
    pub async fn prepare(
        &self,
        run: PrepareRun<'_>,
        log: &(dyn Fn(String) + Send + Sync),
    ) -> Result<SandboxTarget, SandboxRunError> {
        let base_tag = self.ensure_image_and_gateway(log).await?;
        let image = self.image_for(run.project, &base_tag, log).await?;
        let ready = self.ready()?;

        let permit = if run.agent_run {
            match self.inner.slots.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    log("All sandbox slots are busy; this run is queued.".into());
                    Some(
                        self.inner
                            .slots
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|e| sandbox::SandboxError::Other(e.to_string()))?,
                    )
                }
            }
        } else {
            None
        };

        let container = self
            .ensure_workspace_container(run.workspace, &image)
            .await?;

        for repo in run.repos {
            let host_worktree = run.workspace_root.join(&repo.name);
            if transfer::sync_branch_in(
                &ready.docker,
                &container,
                &host_worktree,
                &repo.name,
                &run.workspace.branch,
            )
            .await?
            {
                log(format!(
                    "Copied {} ({}) into the sandbox",
                    repo.name, run.workspace.branch
                ));
            }
        }

        if let Some(issue) = run.issue {
            let prefix = run.project.map(|p| p.id_prefix.as_str()).unwrap_or("KAN");
            let id = issue.display_id(prefix);
            let ticket = format!(
                "# {id}: {}\n\n{}\n\n---\nStart every commit message with `{id}: `.\n",
                issue.title,
                issue.description.as_deref().unwrap_or("")
            );
            ready
                .docker
                .copy_in(
                    &container,
                    CONTEXT_DIR,
                    vec![FileEntry::new("ticket.md", ticket).owned_by_root()],
                )
                .await?;
        }

        let model_token = self
            .inner
            .proxy
            .issue_token(run.execution.id, run.budget_usd, 0.0);
        let egress_token = random_token("kbe-");
        let mut allowed = self
            .inner
            .config
            .read()
            .await
            .sandbox
            .allowed_domains
            .clone();
        if let Some(project) = run.project {
            allowed.extend(project.settings.sandbox_allowed_domains.iter().cloned());
        }
        self.inner.egress_grants.write().unwrap().insert(
            egress_token.clone(),
            EgressGrant {
                run_id: run.execution.id,
                allowlist: Allowlist::with_defaults(allowed),
            },
        );

        let proxy_url = ready
            .network
            .egress_proxy_url(&run.execution.id.simple().to_string(), &egress_token);
        let no_proxy = format!("{GATEWAY_HOST},localhost,127.0.0.1");
        let vars = HashMap::from([
            (
                "ANTHROPIC_BASE_URL".to_string(),
                ready.network.model_proxy_url(),
            ),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), model_token.clone()),
            (
                "OPENAI_BASE_URL".to_string(),
                format!("{}/openai/v1", ready.network.model_proxy_url()),
            ),
            ("OPENAI_API_KEY".to_string(), model_token),
            ("HTTPS_PROXY".to_string(), proxy_url.clone()),
            ("HTTP_PROXY".to_string(), proxy_url.clone()),
            ("https_proxy".to_string(), proxy_url.clone()),
            ("http_proxy".to_string(), proxy_url),
            ("NO_PROXY".to_string(), no_proxy.clone()),
            ("no_proxy".to_string(), no_proxy),
            (
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(),
                "1".to_string(),
            ),
            ("DISABLE_AUTOUPDATER".to_string(), "1".to_string()),
            ("KANBAIN_RUN_ID".to_string(), run.execution.id.to_string()),
        ]);

        ExecutionProcess::set_run_info(
            &self.inner.db.pool,
            run.execution.id,
            run.issue.map(|i| i.id),
            Some(&container),
            Some(&run.workspace.branch),
            run.budget_usd,
        )
        .await?;

        self.inner.runs.lock().await.insert(
            run.execution.id,
            ActiveRun {
                container: container.clone(),
                egress_token,
                _permit: permit,
            },
        );

        Ok(SandboxTarget {
            container,
            host_root: run.workspace_root.to_path_buf(),
            container_root: WORKSPACE_DIR.to_string(),
            user: AGENT_USER.to_string(),
            vars,
            process_key: run.execution.id.to_string(),
        })
    }

    /// Run a headless job (D34): a throwaway container with no repo, the
    /// agent's files, `context` under `/kanbain/context` and the task as
    /// `task.md`. The container takes a concurrency slot, reaches only the
    /// model proxy (and package registries through the egress proxy), and is
    /// removed afterwards. Returns the agent's final message.
    pub async fn run_job(&self, job: JobRun<'_>) -> Result<String, SandboxRunError> {
        let base_tag = self.ensure_image_and_gateway(&|_| {}).await?;
        let image = self.image_for(job.project, &base_tag, &|_| {}).await?;
        let ready = self.ready()?;
        let _permit = self
            .inner
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| sandbox::SandboxError::Other(e.to_string()))?;

        let spec = ContainerSpec {
            name: format!(
                "kanbain-job-{}-{}",
                &job.job_id.simple().to_string()[..12],
                self.inner.instance
            ),
            image,
            labels: HashMap::from([
                (LABEL_MANAGED.to_string(), "true".to_string()),
                (LABEL_INSTANCE.to_string(), self.inner.instance.clone()),
                (LABEL_KIND.to_string(), "job".to_string()),
                (LABEL_EXECUTION.to_string(), job.job_id.to_string()),
            ]),
            limits: Limits {
                cpus: 1.0,
                memory_mb: 2048,
                pids: 512,
            },
            networks: vec![(ready.network.internal_network.clone(), vec![])],
            volumes: vec![KANBAIN_DIR.to_string()],
            working_dir: Some(OUT_DIR.to_string()),
            user: Some(AGENT_USER.to_string()),
            ..Default::default()
        };
        ready.docker.remove(&spec.name).await?;
        let container = ready.docker.create(&spec).await?;
        AgentJob::set_running(&self.inner.db.pool, job.job_id, &container).await?;

        let model_token = self
            .inner
            .proxy
            .issue_token(job.job_id, job.budget_usd, 0.0);
        let egress_token = random_token("kbj-");
        self.inner.egress_grants.write().unwrap().insert(
            egress_token.clone(),
            EgressGrant {
                run_id: job.job_id,
                allowlist: Allowlist::with_defaults(Vec::<String>::new()),
            },
        );

        let result: Result<String, SandboxRunError> = async {
            self.install_agent(&container, job.launch).await?;
            let mut files: Vec<FileEntry> = job
                .context
                .iter()
                .map(|(path, contents)| FileEntry::new(path, contents.clone()).owned_by_root())
                .collect();
            files.push(FileEntry::new(JOB_TASK_FILE, job.task.clone()).owned_by_root());
            ready.docker.copy_in(&container, CONTEXT_DIR, files).await?;

            let task_path = format!("{CONTEXT_DIR}/{JOB_TASK_FILE}");
            let argv = headless::stdin_from(
                job.launch,
                headless::headless_command(job.launch, &task_path),
                &task_path,
            );
            let proxy_url = ready
                .network
                .egress_proxy_url(&job.job_id.simple().to_string(), &egress_token);
            let no_proxy = format!("{GATEWAY_HOST},localhost,127.0.0.1");
            let mut env: Vec<(String, String)> = vec![
                ("ANTHROPIC_BASE_URL".into(), ready.network.model_proxy_url()),
                ("ANTHROPIC_AUTH_TOKEN".into(), model_token.clone()),
                (
                    "OPENAI_BASE_URL".into(),
                    format!("{}/openai/v1", ready.network.model_proxy_url()),
                ),
                ("OPENAI_API_KEY".into(), model_token.clone()),
                ("HTTPS_PROXY".into(), proxy_url.clone()),
                ("https_proxy".into(), proxy_url.clone()),
                ("HTTP_PROXY".into(), proxy_url.clone()),
                ("http_proxy".into(), proxy_url),
                ("NO_PROXY".into(), no_proxy.clone()),
                ("no_proxy".into(), no_proxy),
                (
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
                    "1".into(),
                ),
                ("DISABLE_AUTOUPDATER".into(), "1".into()),
                ("KANBAIN_RUN_ID".into(), job.job_id.to_string()),
            ];
            env.extend(job.launch.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let output = tokio::time::timeout(
                job.timeout,
                ready.docker.exec(
                    &container,
                    &argv_refs,
                    &ExecOptions {
                        user: Some(AGENT_USER.into()),
                        working_dir: Some(OUT_DIR.into()),
                        env,
                    },
                ),
            )
            .await
            .map_err(|_| {
                sandbox::SandboxError::Other(format!(
                    "The agent did not finish within {} minutes",
                    job.timeout.as_secs() / 60
                ))
            })??;
            let stdout = output.stdout_str();
            match headless::parse_headless_output(job.launch.executor, &stdout) {
                Ok(text) => Ok(text),
                Err(e) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let tail: String = stderr
                        .lines()
                        .rev()
                        .take(5)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n");
                    tracing::warn!(
                        "Job {} exited with {}: {e}\n{tail}",
                        job.job_id,
                        output.exit_code
                    );
                    Err(sandbox::SandboxError::Other(if tail.trim().is_empty() {
                        e
                    } else {
                        format!("{e} ({})", tail.trim())
                    })
                    .into())
                }
            }
        }
        .await;

        self.inner.proxy.revoke_run(job.job_id);
        self.inner
            .egress_grants
            .write()
            .unwrap()
            .remove(&egress_token);
        if let Err(e) = ready.docker.remove(&container).await {
            tracing::warn!("Removing job container {container} failed: {e}");
        }
        result
    }

    /// A throwaway container for `claude setup-token` (D33). It reaches
    /// only the Anthropic login hosts, through the egress proxy.
    pub async fn create_login_container(&self) -> Result<LoginContainer, SandboxRunError> {
        let base_tag = self.ensure_image_and_gateway(&|_| {}).await?;
        let ready = self.ready()?;
        let token = random_token("kbl-");
        let spec = ContainerSpec {
            name: format!(
                "kanbain-login-{}-{}",
                &token[4..12].to_ascii_lowercase(),
                self.inner.instance
            ),
            image: base_tag,
            labels: HashMap::from([
                (LABEL_MANAGED.to_string(), "true".to_string()),
                (LABEL_INSTANCE.to_string(), self.inner.instance.clone()),
                (LABEL_KIND.to_string(), "login".to_string()),
            ]),
            limits: Limits {
                cpus: 1.0,
                memory_mb: 1024,
                pids: 256,
            },
            networks: vec![(ready.network.internal_network.clone(), vec![])],
            user: Some(AGENT_USER.to_string()),
            ..Default::default()
        };
        let container = ready.docker.create(&spec).await?;
        self.inner.egress_grants.write().unwrap().insert(
            token.clone(),
            EgressGrant {
                run_id: Uuid::nil(),
                allowlist: Allowlist::new(CLAUDE_LOGIN_HOSTS),
            },
        );
        let proxy_url = ready.network.egress_proxy_url("login", &token);
        let no_proxy = format!("{GATEWAY_HOST},localhost,127.0.0.1");
        Ok(LoginContainer {
            container,
            egress_token: token,
            env: vec![
                ("HTTPS_PROXY".into(), proxy_url.clone()),
                ("https_proxy".into(), proxy_url.clone()),
                ("HTTP_PROXY".into(), proxy_url.clone()),
                ("http_proxy".into(), proxy_url),
                ("NO_PROXY".into(), no_proxy.clone()),
                ("no_proxy".into(), no_proxy),
                ("DISABLE_AUTOUPDATER".into(), "1".into()),
                ("BROWSER".into(), "/bin/false".into()),
            ],
        })
    }

    pub async fn remove_login_container(&self, login: &LoginContainer) {
        self.inner
            .egress_grants
            .write()
            .unwrap()
            .remove(&login.egress_token);
        if let Ok(ready) = self.ready()
            && let Err(e) = ready.docker.remove(&login.container).await
        {
            tracing::warn!("Removing the login container failed: {e}");
        }
    }

    /// Put an agent definition's generated files and skills into the
    /// container, replacing the previous run's. Both are root-owned and
    /// read-only for the agent; `~/.claude/skills` points at the skills,
    /// which Claude Code and OpenCode both scan.
    pub async fn install_agent(
        &self,
        container: &str,
        launch: &AgentLaunch,
    ) -> Result<(), SandboxRunError> {
        let ready = self.ready()?;
        let agent_dir = format!("{KANBAIN_DIR}/{AGENT_DIR}");
        let skills_dir = format!("{KANBAIN_DIR}/{SKILLS_DIR}");
        sandbox::exec_ok(
            &ready.docker,
            container,
            &[
                "sh",
                "-c",
                &format!("rm -rf {agent_dir} {skills_dir} && mkdir -p {agent_dir} {skills_dir}"),
            ],
            &ExecOptions {
                user: Some("root".into()),
                ..Default::default()
            },
        )
        .await?;

        let mut files: Vec<FileEntry> = launch
            .files
            .iter()
            .map(|file| {
                let mut entry = FileEntry::new(&file.path, file.contents.clone()).owned_by_root();
                if file.executable {
                    entry.mode = 0o755;
                }
                entry
            })
            .collect();
        for skill in &launch.skills {
            for file in &skill.files {
                let mut entry = FileEntry::new(
                    format!("{SKILLS_DIR}/{}/{}", skill.name, file.path),
                    file.contents.clone(),
                )
                .owned_by_root();
                // Skills may ship scripts.
                if file.contents.starts_with(b"#!") {
                    entry.mode = 0o755;
                }
                files.push(entry);
            }
        }
        if !files.is_empty() {
            ready.docker.copy_in(container, KANBAIN_DIR, files).await?;
        }
        // The upload leaves existing folders world-writable; the agent must
        // not be able to swap files in them.
        sandbox::exec_ok(
            &ready.docker,
            container,
            &[
                "sh",
                "-c",
                &format!("find {agent_dir} {skills_dir} -type d -exec chmod 755 {{}} +"),
            ],
            &ExecOptions {
                user: Some("root".into()),
                ..Default::default()
            },
        )
        .await?;

        sandbox::exec_ok(
            &ready.docker,
            container,
            &[
                "sh",
                "-c",
                &format!(
                    "mkdir -p \"$HOME/.claude\" && rm -rf \"$HOME/.claude/skills\" \
                     && ln -s {skills_dir} \"$HOME/.claude/skills\""
                ),
            ],
            &ExecOptions {
                user: Some(AGENT_USER.into()),
                ..Default::default()
            },
        )
        .await?;
        Ok(())
    }

    /// Container shell for a workspace terminal: same container as the
    /// workspace's runs, code synced in, package registries reachable
    /// through the egress proxy (no model access).
    pub async fn terminal_shell(
        &self,
        workspace: &Workspace,
        repos: &[Repo],
        workspace_root: &Path,
        host_dir: &Path,
        project: Option<&Project>,
    ) -> Result<crate::pty::ContainerShell, SandboxRunError> {
        let base_tag = self.ensure_image_and_gateway(&|_| {}).await?;
        let image = self.image_for(project, &base_tag, &|_| {}).await?;
        let container = self.ensure_workspace_container(workspace, &image).await?;
        let ready = self.ready()?;
        for repo in repos {
            transfer::sync_branch_in(
                &ready.docker,
                &container,
                &workspace_root.join(&repo.name),
                &repo.name,
                &workspace.branch,
            )
            .await?;
        }

        let token = random_token("kbt-");
        let mut allowed = self
            .inner
            .config
            .read()
            .await
            .sandbox
            .allowed_domains
            .clone();
        if let Some(project) = project {
            allowed.extend(project.settings.sandbox_allowed_domains.iter().cloned());
        }
        self.inner.egress_grants.write().unwrap().insert(
            token.clone(),
            EgressGrant {
                run_id: Uuid::nil(),
                allowlist: Allowlist::with_defaults(allowed),
            },
        );
        let proxy_url = ready.network.egress_proxy_url("terminal", &token);
        let profile = format!(
            "export HTTPS_PROXY='{proxy_url}' HTTP_PROXY='{proxy_url}' https_proxy='{proxy_url}' http_proxy='{proxy_url}' NO_PROXY='{GATEWAY_HOST},localhost,127.0.0.1' no_proxy='{GATEWAY_HOST},localhost,127.0.0.1'\nPS1='sandbox$ '\n"
        );
        ready
            .docker
            .copy_in(
                &container,
                "/home/node",
                vec![FileEntry::new(".kanbain-terminal-env", profile)],
            )
            .await?;

        let target = SandboxTarget {
            container: container.clone(),
            host_root: workspace_root.to_path_buf(),
            container_root: WORKSPACE_DIR.to_string(),
            user: AGENT_USER.to_string(),
            vars: HashMap::new(),
            process_key: String::new(),
        };
        Ok(crate::pty::ContainerShell {
            container,
            dir: target.container_path(host_dir),
            user: AGENT_USER.to_string(),
        })
    }

    /// Stop a run's process group inside its container.
    pub async fn kill(&self, execution_id: Uuid) {
        let Some(container) = self
            .inner
            .runs
            .lock()
            .await
            .get(&execution_id)
            .map(|r| r.container.clone())
        else {
            return;
        };
        let Some(ready) = self.inner.ready.as_ref() else {
            return;
        };
        let pid_file = format!("{SANDBOX_PID_DIR}/{execution_id}.pid");
        sandbox::kill_process_group(&ready.docker, &container, &pid_file).await;
    }

    /// Finish a run: commit what the agent left uncommitted, bring the
    /// branch back to the host worktree, harvest `/kanbain/out`, and revoke
    /// the run's tokens.
    pub async fn finish(
        &self,
        execution_id: Uuid,
        workspace: &Workspace,
        repos: &[Repo],
        workspace_root: &Path,
        sync_out: bool,
        commit_message: &str,
    ) {
        let active = self.inner.runs.lock().await.remove(&execution_id);
        self.inner.proxy.revoke_run(execution_id);
        let Some(active) = active else {
            return;
        };
        self.inner
            .egress_grants
            .write()
            .unwrap()
            .remove(&active.egress_token);
        let Some(ready) = self.inner.ready.as_ref() else {
            return;
        };
        let container = &active.container;
        let pool = &self.inner.db.pool;

        if sync_out {
            for repo in repos {
                let host_worktree = workspace_root.join(&repo.name);
                let result = async {
                    let committed =
                        transfer::commit_all(&ready.docker, container, &repo.name, commit_message)
                            .await?;
                    let outcome = transfer::pull_branch(
                        &ready.docker,
                        container,
                        &host_worktree,
                        &repo.name,
                        &workspace.branch,
                    )
                    .await?;
                    Ok::<_, sandbox::SandboxError>((committed, outcome))
                }
                .await;
                let payload = match result {
                    Ok((committed, PullOutcome::Updated { head })) => json!({
                        "action": "sync_out", "repo": repo.name, "head": head,
                        "committed_leftovers": committed,
                    }),
                    Ok((committed, PullOutcome::UpToDate)) => json!({
                        "action": "sync_out", "repo": repo.name, "up_to_date": true,
                        "committed_leftovers": committed,
                    }),
                    Err(e) => {
                        tracing::error!("Bringing {} back from the sandbox failed: {e}", repo.name);
                        json!({"action": "sync_out", "repo": repo.name, "error": e.to_string()})
                    }
                };
                let _ = ExecutionProcessEvent::create(
                    pool,
                    execution_id,
                    ExecutionProcessEventType::Sandbox,
                    &payload,
                )
                .await;
            }
        }

        if let Ok(files) = ready.docker.copy_out(container, OUT_DIR).await
            && !files.is_empty()
        {
            let files: serde_json::Map<String, serde_json::Value> = files
                .into_iter()
                .map(|(path, contents)| {
                    let path = path.strip_prefix("out/").unwrap_or(&path).to_string();
                    let mut text = String::from_utf8_lossy(&contents).into_owned();
                    if text.len() > MAX_HARVEST_BYTES {
                        let mut cut = MAX_HARVEST_BYTES;
                        while !text.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        text.truncate(cut);
                    }
                    (path, serde_json::Value::String(text))
                })
                .collect();
            let _ = ExecutionProcessEvent::create(
                pool,
                execution_id,
                ExecutionProcessEventType::Sandbox,
                &json!({"action": "harvest", "files": files}),
            )
            .await;
            let _ = ready
                .docker
                .exec(
                    container,
                    &["sh", "-c", &format!("rm -rf {OUT_DIR}/* {OUT_DIR}/.[!.]*")],
                    &ExecOptions {
                        user: Some(AGENT_USER.into()),
                        ..Default::default()
                    },
                )
                .await;
        }
    }

    /// Forward the local ports a sandboxed dev server announces in its log
    /// to the same ports on the host, so the preview proxy reaches it.
    pub fn watch_dev_server_ports(
        &self,
        execution_id: Uuid,
        container: String,
        store: Arc<utils::msg_store::MsgStore>,
    ) {
        use futures::StreamExt;
        use utils::log_msg::LogMsg;

        let runtime = self.clone();
        tokio::spawn(async move {
            let mut forwarded: Vec<u16> = Vec::new();
            let mut stream = store.history_plus_stream();
            while let Some(Ok(msg)) = stream.next().await {
                let text = match msg {
                    LogMsg::Stdout(text) | LogMsg::Stderr(text) => text,
                    LogMsg::Finished => break,
                    _ => continue,
                };
                for port in sandbox::forward::detect_local_ports(&text) {
                    if forwarded.contains(&port) {
                        continue;
                    }
                    forwarded.push(port);
                    match sandbox::forward::forward_port(container.clone(), port).await {
                        Ok(handle) => {
                            runtime
                                .inner
                                .forwards
                                .lock()
                                .await
                                .entry(execution_id)
                                .or_default()
                                .push(handle);
                            store.push_stdout(format!(
                                "[sandbox] Forwarding localhost:{port} to the sandbox for previews\n"
                            ));
                        }
                        Err(e) => store.push_stdout(format!(
                            "[sandbox] Can't forward localhost:{port} for previews: {e}\n"
                        )),
                    }
                }
            }
        });
    }

    /// Revoke a run's tokens and free its slot without syncing anything.
    pub async fn release(&self, execution_id: Uuid) {
        if let Some(handles) = self.inner.forwards.lock().await.remove(&execution_id) {
            for handle in handles {
                handle.abort();
            }
        }
        self.inner.proxy.revoke_run(execution_id);
        if let Some(active) = self.inner.runs.lock().await.remove(&execution_id) {
            self.inner
                .egress_grants
                .write()
                .unwrap()
                .remove(&active.egress_token);
        }
    }

    pub async fn remove_workspace_container(&self, workspace_id: Uuid) {
        if let Some(ready) = self.inner.ready.as_ref()
            && let Ok(ids) = ready
                .docker
                .list_by_labels(&[
                    (LABEL_INSTANCE, self.inner.instance.as_str()),
                    (LABEL_WORKSPACE, workspace_id.to_string().as_str()),
                ])
                .await
        {
            for id in ids {
                if let Err(e) = ready.docker.remove(&id).await {
                    tracing::warn!("Failed to remove sandbox container {id}: {e}");
                }
            }
        }
    }

    pub fn reload_prices(&self, prices: PriceTable) {
        self.inner.proxy.set_prices(prices);
    }
}

fn base_image_args() -> BaseImageArgs {
    BaseImageArgs {
        claude_code_package: CLAUDE_CODE_PACKAGE.into(),
        claude_code_version: CLAUDE_CODE_VERSION.into(),
        opencode_package: OPENCODE_PACKAGE.into(),
        opencode_version: OPENCODE_VERSION.into(),
    }
}

pub fn prices_path() -> PathBuf {
    utils::assets::asset_dir().join("prices.json")
}

/// The editable price table, created from the built-in one on first use.
async fn load_prices() -> PriceTable {
    let path = prices_path();
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match PriceTable::from_json(&json) {
            Ok(table) => return table,
            Err(e) => tracing::error!("{} is invalid, using built-in prices: {e}", path.display()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(e) = tokio::fs::write(&path, model_proxy::prices::DEFAULT_PRICES_JSON).await
            {
                tracing::warn!("Could not write {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("Could not read {}: {e}", path.display()),
    }
    PriceTable::default()
}

/// Real credentials for the model proxy: the encrypted store first, then the
/// backend's own environment. `KANBAIN_ANTHROPIC_BASE_URL` and
/// `KANBAIN_OPENAI_BASE_URL` point the proxy at another upstream.
struct SecretCredentials {
    secrets: Arc<SecretStore>,
}

#[async_trait]
impl model_proxy::Credentials for SecretCredentials {
    async fn upstream(&self, provider: Provider) -> Option<Upstream> {
        let (secret, env_key, base_env, default_base) = match provider {
            Provider::Anthropic => (
                ANTHROPIC_API_KEY,
                "ANTHROPIC_API_KEY",
                "KANBAIN_ANTHROPIC_BASE_URL",
                ANTHROPIC_DEFAULT_BASE_URL,
            ),
            Provider::OpenAi => (
                OPENAI_API_KEY,
                "OPENAI_API_KEY",
                "KANBAIN_OPENAI_BASE_URL",
                OPENAI_DEFAULT_BASE_URL,
            ),
        };
        let base = std::env::var(base_env).unwrap_or_else(|_| default_base.to_string());
        // A connected Claude account takes precedence over an API key.
        if provider == Provider::Anthropic {
            match self.secrets.get(CLAUDE_OAUTH_TOKEN).await {
                Ok(Some(token)) => return Some(StaticCredentials::anthropic_oauth(&base, &token)),
                Ok(None) => {}
                Err(e) => tracing::error!("Reading the Claude account token failed: {e}"),
            }
        }
        let key = match self.secrets.get(secret).await {
            Ok(Some(key)) => key,
            Ok(None) => std::env::var(env_key).ok()?,
            Err(e) => {
                tracing::error!("Reading the {secret} secret failed: {e}");
                return None;
            }
        };
        Some(match provider {
            Provider::Anthropic => StaticCredentials::anthropic_api_key(&base, &key),
            Provider::OpenAi => StaticCredentials::bearer(&base, &key),
        })
    }
}

/// Stores model calls on the run and reports budget exhaustion.
struct RunEvents {
    db: DBService,
    budget_tx: mpsc::UnboundedSender<Uuid>,
}

#[async_trait]
impl ProxyEvents for RunEvents {
    async fn model_call(&self, record: ModelCallRecord) {
        let pool = &self.db.pool;
        let usage = record.usage;
        let delta = RunUsageDelta {
            tokens_in: usage.input_tokens as i64,
            tokens_out: usage.output_tokens as i64,
            tokens_cache_read: usage.cache_read_tokens as i64,
            tokens_cache_write: usage.cache_write_tokens() as i64,
            cost_usd: record.cost_usd.unwrap_or(0.0),
        };
        // Headless jobs (intake, history) keep their usage on the job.
        match AgentJob::add_usage(pool, record.run_id, &delta).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => tracing::warn!("Recording usage for job {} failed: {e}", record.run_id),
        }
        if let Err(e) = ExecutionProcess::add_usage(pool, record.run_id, delta).await {
            tracing::warn!("Recording usage for run {} failed: {e}", record.run_id);
        }
        let payload = serde_json::to_value(&record).unwrap_or_default();
        if let Err(e) = ExecutionProcessEvent::create(
            pool,
            record.run_id,
            ExecutionProcessEventType::ModelCall,
            &payload,
        )
        .await
        {
            tracing::warn!(
                "Recording a model call for run {} failed: {e}",
                record.run_id
            );
        }
    }

    async fn budget_exhausted(&self, run_id: Uuid, spent_usd: f64) {
        let _ = ExecutionProcessEvent::create(
            &self.db.pool,
            run_id,
            ExecutionProcessEventType::BudgetExceeded,
            &json!({"spent_usd": spent_usd}),
        )
        .await;
        let _ = self.budget_tx.send(run_id);
    }
}

/// Maps egress credentials to runs and logs what gets blocked.
struct RunEgressPolicy {
    db: DBService,
    grants: Arc<std::sync::RwLock<HashMap<String, EgressGrant>>>,
}

impl RunEgressPolicy {
    async fn log_block(&self, run_id: Uuid, host: &str, port: u16, reason: &str) {
        tracing::info!("Sandbox egress blocked for run {run_id}: {host}:{port} ({reason})");
        if run_id.is_nil() {
            return;
        }
        let _ = ExecutionProcessEvent::create(
            &self.db.pool,
            run_id,
            ExecutionProcessEventType::EgressBlock,
            &json!({"host": host, "port": port, "reason": reason}),
        )
        .await;
    }
}

#[async_trait]
impl EgressPolicy for RunEgressPolicy {
    async fn check(
        &self,
        _user: &str,
        token: &str,
        host: &str,
        port: u16,
        plain_http: bool,
    ) -> EgressVerdict {
        let (run_id, allowed) = {
            let grants = self.grants.read().unwrap();
            let Some(grant) = grants.get(token) else {
                return EgressVerdict::Unauthorized;
            };
            (grant.run_id, grant.allowlist.allows(host))
        };
        let reason = if plain_http {
            "plain HTTP is not allowed".to_string()
        } else if port != 443 {
            format!("port {port} is not allowed")
        } else if !allowed {
            format!("{host} is not on the allowlist")
        } else {
            return EgressVerdict::Allowed;
        };
        self.log_block(run_id, host, port, &reason).await;
        EgressVerdict::Blocked { reason }
    }

    async fn report_blocked(&self, user: &str, host: &str, port: u16, reason: &str) {
        // Runs use their execution id as the proxy user; terminals use "terminal".
        let run_id = Uuid::parse_str(user).unwrap_or_default();
        self.log_block(run_id, host, port, reason).await;
    }
}
