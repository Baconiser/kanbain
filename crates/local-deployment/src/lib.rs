use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use client_info::ClientInfo;
use db::DBService;
use deployment::{Deployment, DeploymentError};
use executors::profile::ExecutorConfigs;
use git::GitService;
use preview_proxy::PreviewProxyService;
use services::services::{
    approvals::Approvals,
    config::{Config, load_config_from_file, save_config_to_file},
    container::ContainerService,
    events::EventService,
    file::FileService,
    file_search::FileSearchCache,
    filesystem::FilesystemService,
    pr_monitor::PrMonitorService,
    queued_message::QueuedMessageService,
    repo::RepoService,
};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use utils::{assets::config_path, msg_store::MsgStore};
use workspace_manager::WorkspaceManager;
use worktree_manager::WorktreeManager;

use crate::{container::LocalContainerService, pty::PtyService};
pub mod claude_login;
mod command;
pub mod container;
mod copy;
pub mod history;
pub mod intake;
pub mod jobs;
pub mod pty;
pub mod sandbox;

#[derive(Clone)]
pub struct LocalDeployment {
    config: Arc<RwLock<Config>>,
    db: DBService,
    workspace_manager: WorkspaceManager,
    container: LocalContainerService,
    git: GitService,
    repo: RepoService,
    file: FileService,
    filesystem: FilesystemService,
    events: EventService,
    file_search_cache: Arc<FileSearchCache>,
    approvals: Approvals,
    queued_message_service: QueuedMessageService,
    client_info: ClientInfo,
    preview_proxy: PreviewProxyService,
    pty: PtyService,
    pr_sync_notify: Arc<Notify>,
    secrets: Arc<services::services::secrets::SecretStore>,
    agent_library: Arc<services::services::agent_library::AgentLibrary>,
    claude_login: claude_login::ClaudeLogin,
    jobs: jobs::AgentJobs,
    intake: intake::IntakeService,
    history: history::HistoryService,
}

#[async_trait]
impl Deployment for LocalDeployment {
    async fn new(_shutdown: CancellationToken) -> Result<Self, DeploymentError> {
        // Run one-time process logs migration from DB to filesystem
        services::services::execution_process::migrate_execution_logs_to_files()
            .await
            .map_err(|e| DeploymentError::Other(anyhow::anyhow!("Migration failed: {}", e)))?;

        let mut raw_config = load_config_from_file(&config_path()).await;

        let profiles = ExecutorConfigs::get_cached();
        if !raw_config.onboarding_acknowledged
            && let Ok(recommended_executor) = profiles.get_recommended_executor_profile().await
        {
            raw_config.executor_profile = recommended_executor;
        }

        // Check if app version has changed and set release notes flag
        {
            let current_version = utils::version::APP_VERSION;
            let stored_version = raw_config.last_app_version.as_deref();

            if stored_version != Some(current_version) {
                // Show release notes only if this is an upgrade (not first install)
                raw_config.show_release_notes = stored_version.is_some();
                raw_config.last_app_version = Some(current_version.to_string());
            }
        }

        // Always save config (may have been migrated or version updated)
        save_config_to_file(&raw_config, &config_path()).await?;

        if let Some(workspace_dir) = &raw_config.workspace_dir {
            let path = utils::path::expand_tilde(workspace_dir);
            WorktreeManager::set_workspace_dir_override(path);
        }

        let config = Arc::new(RwLock::new(raw_config));
        let git = GitService::new();
        let repo = RepoService::new();
        let msg_stores = Arc::new(RwLock::new(HashMap::new()));
        let filesystem = FilesystemService::new();

        // Create shared components for EventService
        let events_msg_store = Arc::new(MsgStore::new());
        let events_entry_count = Arc::new(RwLock::new(0));

        // Create DB with event hooks
        let db = {
            let hook = EventService::create_hook(
                events_msg_store.clone(),
                events_entry_count.clone(),
                DBService::new().await?, // Temporary DB service for the hook
            );
            DBService::new_with_after_connect(hook).await?
        };

        let file = FileService::new(db.clone().pool)?;
        {
            let file_service = file.clone();
            tokio::spawn(async move {
                tracing::info!("Starting orphaned file cleanup...");
                if let Err(e) = file_service.delete_orphaned_files().await {
                    tracing::error!("Failed to clean up orphaned files: {}", e);
                }
            });
        }

        {
            // Daily SQLite backup (checked hourly), pruned to the configured count.
            let pool = db.pool.clone();
            let config = config.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
                loop {
                    interval.tick().await;
                    let keep = config.read().await.backup_keep;
                    match db::backup::run_daily_backup(&pool, &db::backup::backups_dir(), keep)
                        .await
                    {
                        Ok(Some(path)) => {
                            tracing::info!("Daily database backup: {}", path.display())
                        }
                        Ok(None) => {}
                        Err(e) => tracing::error!("Daily database backup failed: {}", e),
                    }
                }
            });
        }

        let approvals = Approvals::new();
        let queued_message_service = QueuedMessageService::new();

        let client_info = ClientInfo::new();
        let preview_proxy = PreviewProxyService::new();

        let secrets = Arc::new(services::services::secrets::SecretStore::new(
            utils::assets::asset_dir(),
        ));
        let (sandbox, budget_rx) =
            crate::sandbox::SandboxRuntime::new(db.clone(), config.clone(), secrets.clone()).await;

        let agent_library = services::services::agent_library::AgentLibrary::new(
            utils::assets::asset_dir(),
            db.pool.clone(),
        );
        if let Err(e) = agent_library.init().await {
            tracing::error!("Failed to load the agent library: {e}");
        }

        let claude_login = claude_login::ClaudeLogin::new(sandbox.clone(), secrets.clone());
        let jobs = jobs::AgentJobs::new(db.clone(), sandbox.clone(), agent_library.clone());
        let intake = intake::IntakeService::new(db.clone(), file.clone(), jobs.clone());
        // Jobs and intakes of a previous launch can never finish.
        if let Err(e) = db::models::agent_job::AgentJob::fail_interrupted(&db.pool).await {
            tracing::warn!("Closing interrupted agent jobs failed: {e}");
        }
        if let Err(e) = db::models::issue_history::IssueHistory::fail_interrupted(&db.pool).await {
            tracing::warn!("Closing interrupted history analyses failed: {e}");
        }
        if let Err(e) = sqlx::query(
            "UPDATE ticket_intakes SET status = 'failed', \
             error = 'Interrupted by an app restart' WHERE status = 'enhancing'",
        )
        .execute(&db.pool)
        .await
        {
            tracing::warn!("Closing interrupted intakes failed: {e}");
        }

        let workspace_manager = WorkspaceManager::new(db.clone());
        let container = LocalContainerService::new(
            db.clone(),
            workspace_manager.clone(),
            msg_stores.clone(),
            config.clone(),
            git.clone(),
            file.clone(),
            approvals.clone(),
            queued_message_service.clone(),
            sandbox,
            budget_rx,
            agent_library.clone(),
        )
        .await;

        let history = history::HistoryService::new(db.clone(), jobs.clone());
        container.set_history(history.clone());

        let events = EventService::new(db.clone(), events_msg_store, events_entry_count);

        let file_search_cache = Arc::new(FileSearchCache::new());

        let pty = PtyService::new();
        let pr_sync_notify = Arc::new(Notify::new());
        {
            let db = db.clone();
            let container = container.clone();
            PrMonitorService::spawn(db, container, pr_sync_notify.clone()).await;
        }

        let deployment = Self {
            config,
            db,
            workspace_manager,
            container,
            git,
            repo,
            file,
            filesystem,
            events,
            file_search_cache,
            approvals,
            queued_message_service,
            client_info,
            preview_proxy,
            pty,
            pr_sync_notify,
            secrets,
            agent_library,
            claude_login,
            jobs,
            intake,
            history,
        };

        Ok(deployment)
    }

    fn config(&self) -> &Arc<RwLock<Config>> {
        &self.config
    }

    fn db(&self) -> &DBService {
        &self.db
    }

    fn container(&self) -> &impl ContainerService {
        &self.container
    }

    fn git(&self) -> &GitService {
        &self.git
    }

    fn repo(&self) -> &RepoService {
        &self.repo
    }

    fn file(&self) -> &FileService {
        &self.file
    }

    fn filesystem(&self) -> &FilesystemService {
        &self.filesystem
    }

    fn events(&self) -> &EventService {
        &self.events
    }

    fn file_search_cache(&self) -> &Arc<FileSearchCache> {
        &self.file_search_cache
    }

    fn approvals(&self) -> &Approvals {
        &self.approvals
    }

    fn queued_message_service(&self) -> &QueuedMessageService {
        &self.queued_message_service
    }

    fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    fn preview_proxy(&self) -> &PreviewProxyService {
        &self.preview_proxy
    }
}

impl LocalDeployment {
    pub fn workspace_manager(&self) -> &WorkspaceManager {
        &self.workspace_manager
    }

    pub fn pty(&self) -> &PtyService {
        &self.pty
    }

    pub fn sandbox(&self) -> &crate::sandbox::SandboxRuntime {
        self.container.sandbox()
    }

    pub fn secrets(&self) -> &Arc<services::services::secrets::SecretStore> {
        &self.secrets
    }

    pub fn agent_library(&self) -> &Arc<services::services::agent_library::AgentLibrary> {
        &self.agent_library
    }

    pub fn claude_login(&self) -> &claude_login::ClaudeLogin {
        &self.claude_login
    }

    pub fn jobs(&self) -> &jobs::AgentJobs {
        &self.jobs
    }

    pub fn intake(&self) -> &intake::IntakeService {
        &self.intake
    }

    pub fn history(&self) -> &history::HistoryService {
        &self.history
    }

    pub fn trigger_pr_sync(&self) {
        self.pr_sync_notify.notify_one();
    }
}
