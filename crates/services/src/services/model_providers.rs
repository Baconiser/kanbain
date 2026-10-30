//! Custom OpenAI-compatible model providers (DeepSeek, OpenRouter, a local
//! vLLM, …) added in the settings.
//!
//! The definitions live in `<data>/model_providers.json`; each provider's API
//! key is sealed in the [`SecretStore`](super::secrets::SecretStore) under
//! [`api_key_secret`]. The model proxy forwards `/p/<id>/*` to the provider's
//! base URL with that key, and OpenCode agents pick them as `<id>/<model>`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use ts_rs::TS;

/// Provider ids with a meaning of their own in agent model strings.
const RESERVED_IDS: &[&str] = &["anthropic", "openai", "kanbain-openai"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CustomModelProvider {
    /// Lowercase slug; agents name models as `<id>/<model>`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// OpenAI-compatible base URL, e.g. `https://api.deepseek.com/v1`.
    /// Requests go to `<base_url>/chat/completions`.
    pub base_url: String,
    /// Model ids offered in the agent editor.
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ModelProviderError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

/// The secret holding a provider's API key.
pub fn api_key_secret(id: &str) -> String {
    format!("provider_api_key:{id}")
}

impl CustomModelProvider {
    /// Trim fields, drop empty and duplicate models, and check the rest.
    pub fn normalized(mut self) -> Result<Self, ModelProviderError> {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        let mut models: Vec<String> = Vec::new();
        for model in self.models.iter().map(|m| m.trim()) {
            if !model.is_empty() && !models.iter().any(|m| m == model) {
                models.push(model.to_string());
            }
        }
        self.models = models;

        let invalid = |m: &str| Err(ModelProviderError::Invalid(m.to_string()));
        if self.id.is_empty()
            || self.id.len() > 32
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || self.id.starts_with('-')
        {
            return invalid("The id must be 1-32 lowercase letters, digits or dashes.");
        }
        if RESERVED_IDS.contains(&self.id.as_str()) {
            return invalid("This id is reserved; pick another one.");
        }
        if self.name.is_empty() {
            self.name = self.id.clone();
        }
        let Ok(url) = url::Url::parse(&self.base_url) else {
            return invalid("The base URL is not a valid URL.");
        };
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return invalid("The base URL must start with http:// or https://.");
        }
        if url.query().is_some() || url.fragment().is_some() {
            return invalid("The base URL must not have a query or fragment.");
        }
        Ok(self)
    }
}

pub struct ModelProviderStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl ModelProviderStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join("model_providers.json"),
            lock: Mutex::new(()),
        }
    }

    async fn load(&self) -> Result<Vec<CustomModelProvider>, ModelProviderError> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    async fn save(&self, providers: &[CustomModelProvider]) -> Result<(), ModelProviderError> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.path.with_extension("tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(providers)?).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<CustomModelProvider>, ModelProviderError> {
        let _guard = self.lock.lock().await;
        self.load().await
    }

    pub async fn get(&self, id: &str) -> Result<Option<CustomModelProvider>, ModelProviderError> {
        Ok(self.list().await?.into_iter().find(|p| p.id == id))
    }

    /// Add a provider, or replace the one with the same id.
    pub async fn upsert(
        &self,
        provider: CustomModelProvider,
    ) -> Result<CustomModelProvider, ModelProviderError> {
        let provider = provider.normalized()?;
        let _guard = self.lock.lock().await;
        let mut all = self.load().await?;
        match all.iter_mut().find(|p| p.id == provider.id) {
            Some(existing) => *existing = provider.clone(),
            None => all.push(provider.clone()),
        }
        self.save(&all).await?;
        Ok(provider)
    }

    /// Returns whether a provider was removed.
    pub async fn remove(&self, id: &str) -> Result<bool, ModelProviderError> {
        let _guard = self.lock.lock().await;
        let mut all = self.load().await?;
        let before = all.len();
        all.retain(|p| p.id != id);
        if all.len() == before {
            return Ok(false);
        }
        self.save(&all).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> CustomModelProvider {
        CustomModelProvider {
            id: id.into(),
            name: " DeepSeek ".into(),
            base_url: "https://api.deepseek.com/v1/".into(),
            models: vec!["deepseek-chat".into(), " ".into(), "deepseek-chat".into()],
        }
    }

    #[test]
    fn normalizes_and_validates() {
        let p = provider("deepseek").normalized().unwrap();
        assert_eq!(p.name, "DeepSeek");
        assert_eq!(p.base_url, "https://api.deepseek.com/v1");
        assert_eq!(p.models, vec!["deepseek-chat"]);

        for bad in ["", "Deep", "a/b", "openai", "-x"] {
            assert!(provider(bad).normalized().is_err(), "{bad}");
        }
        let mut p = provider("x");
        p.base_url = "ftp://host".into();
        assert!(p.normalized().is_err());
    }

    #[tokio::test]
    async fn upsert_replaces_and_remove_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelProviderStore::new(dir.path());
        store.upsert(provider("deepseek")).await.unwrap();
        let mut changed = provider("deepseek");
        changed.models = vec!["deepseek-reasoner".into()];
        store.upsert(changed).await.unwrap();
        store.upsert(provider("local")).await.unwrap();
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].models, vec!["deepseek-reasoner"]);
        assert!(store.remove("deepseek").await.unwrap());
        assert!(!store.remove("deepseek").await.unwrap());
        assert_eq!(store.list().await.unwrap().len(), 1);
    }
}
