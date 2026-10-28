use anyhow::Error;
use executors::{executors::BaseCodingAgent, profile::ExecutorProfileId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
pub use v7::{
    EditorConfig, EditorType, GitHubConfig, NotificationConfig, ShowcaseState, SoundFile,
    ThemeMode, UiLanguage,
};

use crate::services::config::versions::v7;

fn default_git_branch_prefix() -> String {
    "kanbain".to_string()
}

/// Branch prefix inherited from Vibe Kanban's default.
const UPSTREAM_GIT_BRANCH_PREFIX: &str = "vk";

fn default_pr_auto_description_enabled() -> bool {
    true
}

fn default_commit_reminder_enabled() -> bool {
    true
}

fn default_relay_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum SendMessageShortcut {
    #[default]
    ModifierEnter,
    Enter,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Config {
    pub config_version: String,
    pub theme: ThemeMode,
    pub executor_profile: ExecutorProfileId,
    pub disclaimer_acknowledged: bool,
    pub onboarding_acknowledged: bool,
    #[serde(default)]
    pub remote_onboarding_acknowledged: bool,
    pub notifications: NotificationConfig,
    pub editor: EditorConfig,
    pub github: GitHubConfig,
    pub analytics_enabled: bool,
    pub workspace_dir: Option<String>,
    pub last_app_version: Option<String>,
    pub show_release_notes: bool,
    #[serde(default)]
    pub language: UiLanguage,
    #[serde(default = "default_git_branch_prefix")]
    pub git_branch_prefix: String,
    #[serde(default)]
    pub showcases: ShowcaseState,
    #[serde(default = "default_pr_auto_description_enabled")]
    pub pr_auto_description_enabled: bool,
    #[serde(default)]
    pub pr_auto_description_prompt: Option<String>,
    #[serde(default = "default_commit_reminder_enabled")]
    pub commit_reminder_enabled: bool,
    #[serde(default)]
    pub commit_reminder_prompt: Option<String>,
    #[serde(default)]
    pub send_message_shortcut: SendMessageShortcut,
    #[serde(default = "default_relay_enabled")]
    pub relay_enabled: bool,
    #[serde(default)]
    pub host_nickname: Option<String>,
    /// Number of daily database backups to keep.
    #[serde(default = "default_backup_keep")]
    pub backup_keep: u32,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    /// Embedders, reranker and retrieval defaults (Settings → Search).
    #[serde(default)]
    pub search: embeddings::SearchConfig,
}

/// Docker sandbox settings for agent runs (D28, D35).
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(default)]
pub struct SandboxConfig {
    /// Run agents and workspace scripts in Docker containers. Turning this
    /// off runs them directly on this machine.
    pub enabled: bool,
    /// Agent runs allowed at the same time; the rest wait in a queue.
    pub max_concurrent_runs: u32,
    pub cpus: f64,
    pub memory_mb: u32,
    pub pids_limit: u32,
    /// Model budget per run, enforced by the model proxy.
    pub run_budget_usd: Option<f64>,
    /// Default model budget per ticket, unless the ticket sets its own.
    pub ticket_budget_usd: Option<f64>,
    /// Longest an agent may run in one execution.
    pub max_run_minutes: u32,
    /// Extra hosts every sandbox may reach.
    pub allowed_domains: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent_runs: 3,
            cpus: 2.0,
            memory_mb: 4096,
            pids_limit: 1024,
            run_budget_usd: Some(2.0),
            ticket_budget_usd: Some(5.0),
            max_run_minutes: 30,
            allowed_domains: Vec::new(),
        }
    }
}

fn default_backup_keep() -> u32 {
    db::backup::DEFAULT_DAILY_KEEP
}

impl Config {
    fn from_v7_config(old_config: v7::Config) -> Self {
        // Convert Option<bool> to bool: None or Some(true) become true, Some(false) stays false
        let analytics_enabled = old_config.analytics_enabled.unwrap_or(true);

        Self {
            config_version: "v8".to_string(),
            theme: old_config.theme,
            executor_profile: old_config.executor_profile,
            disclaimer_acknowledged: old_config.disclaimer_acknowledged,
            onboarding_acknowledged: old_config.onboarding_acknowledged,
            remote_onboarding_acknowledged: false,
            notifications: old_config.notifications,
            editor: old_config.editor,
            github: old_config.github,
            analytics_enabled,
            workspace_dir: old_config.workspace_dir,
            last_app_version: old_config.last_app_version,
            show_release_notes: old_config.show_release_notes,
            language: old_config.language,
            git_branch_prefix: if old_config.git_branch_prefix == UPSTREAM_GIT_BRANCH_PREFIX {
                default_git_branch_prefix()
            } else {
                old_config.git_branch_prefix
            },
            showcases: old_config.showcases,
            pr_auto_description_enabled: true,
            pr_auto_description_prompt: None,
            commit_reminder_enabled: true,
            commit_reminder_prompt: None,
            send_message_shortcut: SendMessageShortcut::default(),
            relay_enabled: true,
            host_nickname: None,
            backup_keep: default_backup_keep(),
            sandbox: SandboxConfig::default(),
            search: embeddings::SearchConfig::default(),
        }
    }

    pub fn from_previous_version(raw_config: &str) -> Result<Self, Error> {
        let old_config = v7::Config::from(raw_config.to_string());
        Ok(Self::from_v7_config(old_config))
    }
}

impl From<String> for Config {
    fn from(raw_config: String) -> Self {
        if let Ok(mut config) = serde_json::from_str::<Config>(&raw_config)
            && config.config_version == "v8"
        {
            if config.git_branch_prefix == UPSTREAM_GIT_BRANCH_PREFIX {
                config.git_branch_prefix = default_git_branch_prefix();
            }
            return config;
        }

        match Self::from_previous_version(&raw_config) {
            Ok(config) => {
                tracing::info!("Config upgraded to v8");
                config
            }
            Err(e) => {
                tracing::warn!("Config migration failed: {}, using default", e);
                Self::default()
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: "v8".to_string(),
            theme: ThemeMode::System,
            executor_profile: ExecutorProfileId::new(BaseCodingAgent::ClaudeCode),
            disclaimer_acknowledged: false,
            onboarding_acknowledged: false,
            remote_onboarding_acknowledged: false,
            notifications: NotificationConfig::default(),
            editor: EditorConfig::default(),
            github: GitHubConfig::default(),
            analytics_enabled: true,
            workspace_dir: None,
            last_app_version: None,
            show_release_notes: false,
            language: UiLanguage::default(),
            git_branch_prefix: default_git_branch_prefix(),
            showcases: ShowcaseState::default(),
            pr_auto_description_enabled: true,
            pr_auto_description_prompt: None,
            commit_reminder_enabled: true,
            commit_reminder_prompt: None,
            send_message_shortcut: SendMessageShortcut::default(),
            relay_enabled: true,
            host_nickname: None,
            backup_keep: default_backup_keep(),
            sandbox: SandboxConfig::default(),
            search: embeddings::SearchConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_branch_prefix_becomes_kanbain() {
        let mut config = Config::default();
        assert_eq!(config.git_branch_prefix, "kanbain");

        config.git_branch_prefix = "vk".to_string();
        let loaded = Config::from(serde_json::to_string(&config).unwrap());
        assert_eq!(loaded.git_branch_prefix, "kanbain");

        config.git_branch_prefix = "team".to_string();
        let loaded = Config::from(serde_json::to_string(&config).unwrap());
        assert_eq!(loaded.git_branch_prefix, "team");
    }
}
