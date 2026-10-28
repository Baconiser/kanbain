//! Encrypted credential storage in the data folder.
//!
//! Secrets are sealed with AES-256-GCM under a key generated on first use
//! and kept in `<data>/secret.key` (owner-only on Unix). This keeps API keys
//! out of plain-text config and backups of `secrets.json`; the OS keychain
//! comes with the login work in Phase 2.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use aws_lc_rs::aead::{AES_256_GCM, Aad, Nonce, RandomizedNonceKey};
use base64::{Engine, engine::general_purpose::STANDARD};
use thiserror::Error;
use tokio::sync::Mutex;

pub const ANTHROPIC_API_KEY: &str = "anthropic_api_key";
pub const OPENAI_API_KEY: &str = "openai_api_key";
/// Long-lived Claude account token from `claude setup-token` (D33).
pub const CLAUDE_OAUTH_TOKEN: &str = "claude_oauth_token";

const NONCE_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("secret store is corrupt or the key changed")]
    Crypto,
}

pub struct SecretStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl SecretStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            lock: Mutex::new(()),
        }
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("secret.key")
    }

    fn store_path(&self) -> PathBuf {
        self.dir.join("secrets.json")
    }

    async fn key(&self) -> Result<RandomizedNonceKey, SecretsError> {
        let path = self.key_path();
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) if bytes.len() == 32 => bytes,
            Ok(_) => return Err(SecretsError::Crypto),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 32];
                aws_lc_rs::rand::fill(&mut bytes).map_err(|_| SecretsError::Crypto)?;
                write_private(&path, &bytes).await?;
                bytes
            }
            Err(e) => return Err(e.into()),
        };
        RandomizedNonceKey::new(&AES_256_GCM, &bytes).map_err(|_| SecretsError::Crypto)
    }

    async fn load(&self) -> Result<BTreeMap<String, String>, SecretsError> {
        match tokio::fs::read(self.store_path()).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
        let _guard = self.lock.lock().await;
        let Some(sealed) = self.load().await?.remove(name) else {
            return Ok(None);
        };
        let mut data = STANDARD.decode(sealed).map_err(|_| SecretsError::Crypto)?;
        if data.len() < NONCE_LEN {
            return Err(SecretsError::Crypto);
        }
        let mut ciphertext = data.split_off(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(&data).map_err(|_| SecretsError::Crypto)?;
        let key = self.key().await?;
        let plain = key
            .open_in_place(nonce, Aad::from(name.as_bytes()), &mut ciphertext)
            .map_err(|_| SecretsError::Crypto)?;
        String::from_utf8(plain.to_vec())
            .map(Some)
            .map_err(|_| SecretsError::Crypto)
    }

    pub async fn contains(&self, name: &str) -> Result<bool, SecretsError> {
        let _guard = self.lock.lock().await;
        Ok(self.load().await?.contains_key(name))
    }

    pub async fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        let _guard = self.lock.lock().await;
        let key = self.key().await?;
        let mut data = value.as_bytes().to_vec();
        let nonce = key
            .seal_in_place_append_tag(Aad::from(name.as_bytes()), &mut data)
            .map_err(|_| SecretsError::Crypto)?;
        let mut sealed = nonce.as_ref().to_vec();
        sealed.extend_from_slice(&data);
        let mut all = self.load().await?;
        all.insert(name.to_string(), STANDARD.encode(sealed));
        write_private(&self.store_path(), &serde_json::to_vec_pretty(&all)?).await?;
        Ok(())
    }

    pub async fn remove(&self, name: &str) -> Result<(), SecretsError> {
        let _guard = self.lock.lock().await;
        let mut all = self.load().await?;
        if all.remove(name).is_some() {
            write_private(&self.store_path(), &serde_json::to_vec_pretty(&all)?).await?;
        }
        Ok(())
    }
}

async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await?;
    }
    tokio::fs::rename(&tmp, path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_and_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(dir.path());
        assert_eq!(store.get(ANTHROPIC_API_KEY).await.unwrap(), None);
        store.set(ANTHROPIC_API_KEY, "sk-ant-test").await.unwrap();
        assert_eq!(
            store.get(ANTHROPIC_API_KEY).await.unwrap().as_deref(),
            Some("sk-ant-test")
        );
        let raw = std::fs::read_to_string(dir.path().join("secrets.json")).unwrap();
        assert!(!raw.contains("sk-ant-test"));

        // A ciphertext moved to another name fails authentication.
        let mut all: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap();
        let sealed = all.remove(ANTHROPIC_API_KEY).unwrap();
        all.insert(OPENAI_API_KEY.into(), sealed);
        std::fs::write(
            dir.path().join("secrets.json"),
            serde_json::to_vec(&all).unwrap(),
        )
        .unwrap();
        assert!(store.get(OPENAI_API_KEY).await.is_err());

        store.remove(OPENAI_API_KEY).await.unwrap();
        assert!(!store.contains(OPENAI_API_KEY).await.unwrap());
    }
}
