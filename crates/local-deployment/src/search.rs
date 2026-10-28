//! Wires the search index (Phase 3c) into the app: its file in the data
//! folder, API keys from the encrypted secret store, and index jobs from
//! the ticket change stream (E18).

use std::sync::Arc;

use async_trait::async_trait;
use db::DBService;
use embeddings::{SearchConfig, SearchService, service::ApiKeys};
use json_patch::PatchOperation;
use services::services::secrets::SecretStore;
use tokio::sync::RwLock;
use utils::{log_msg::LogMsg, msg_store::MsgStore};
use uuid::Uuid;

struct SecretKeys(Arc<SecretStore>);

#[async_trait]
impl ApiKeys for SecretKeys {
    async fn key(&self, slot: &str) -> Option<String> {
        match self.0.get(slot).await {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!("Reading the {slot} secret failed: {e}");
                None
            }
        }
    }
}

/// Opens `embeddings.sqlite` (deleting and rebuilding it when it can't be
/// opened: it is derived data, E3) and starts the index worker.
pub async fn start_search(
    db: &DBService,
    config: SearchConfig,
    secrets: Arc<SecretStore>,
    msg_store: Arc<MsgStore>,
) -> Option<SearchService> {
    let path = utils::assets::asset_dir().join("embeddings.sqlite");
    let config = Arc::new(RwLock::new(config));
    let keys: Arc<dyn ApiKeys> = Arc::new(SecretKeys(secrets));
    let service =
        match SearchService::open(&path, db.pool.clone(), config.clone(), keys.clone()).await {
            Ok(service) => service,
            Err(e) => {
                tracing::error!("Opening {} failed ({e}); rebuilding it", path.display());
                for suffix in ["", "-wal", "-shm"] {
                    let _ = tokio::fs::remove_file(format!("{}{suffix}", path.display())).await;
                }
                match SearchService::open(&path, db.pool.clone(), config, keys).await {
                    Ok(service) => service,
                    Err(e) => {
                        tracing::error!("Search is unavailable: {e}");
                        return None;
                    }
                }
            }
        };
    service.start();
    spawn_change_listener(service.clone(), msg_store);
    Some(service)
}

/// What a ticket-table patch means for the index.
#[derive(Debug, PartialEq, Eq)]
pub enum IndexSignal {
    Issue(Uuid),
    CommentRemoved(Uuid),
}

/// Ticket, comment and timeline patches name the ticket to re-index.
pub fn signals(patch: &json_patch::Patch) -> Vec<IndexSignal> {
    let mut out = Vec::new();
    for op in &patch.0 {
        let path = op.path().to_string();
        let mut parts = path.trim_start_matches('/').split('/');
        let (Some(collection), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(id) else {
            continue;
        };
        let value = match op {
            PatchOperation::Add(a) => Some(&a.value),
            PatchOperation::Replace(r) => Some(&r.value),
            _ => None,
        };
        let issue_of = |value: Option<&serde_json::Value>| {
            value
                .and_then(|v| v.get("issue_id"))
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
        };
        match (collection, value) {
            ("issues", _) => out.push(IndexSignal::Issue(id)),
            ("comments", None) => out.push(IndexSignal::CommentRemoved(id)),
            ("comments" | "events", value) => {
                if let Some(issue) = issue_of(value) {
                    out.push(IndexSignal::Issue(issue));
                }
            }
            _ => {}
        }
    }
    out
}

fn spawn_change_listener(service: SearchService, msg_store: Arc<MsgStore>) {
    let mut receiver = msg_store.get_receiver();
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(LogMsg::JsonPatch(patch)) => {
                    for signal in signals(&patch) {
                        match signal {
                            IndexSignal::Issue(id) => service.enqueue_issue(id).await,
                            IndexSignal::CommentRemoved(id) => service.comment_removed(id).await,
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!("Search missed {missed} changes; re-checking all tickets");
                    let _ = service
                        .reindex(&embeddings::service::ReindexRequest::default())
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn patches_become_signals() {
        let issue = Uuid::new_v4();
        let comment = Uuid::new_v4();
        let patch: json_patch::Patch = serde_json::from_value(json!([
            { "op": "replace", "path": format!("/issues/{issue}"), "value": {} },
            { "op": "add", "path": format!("/comments/{comment}"), "value": { "issue_id": issue } },
            { "op": "remove", "path": format!("/comments/{comment}") },
            { "op": "add", "path": format!("/events/{}", Uuid::new_v4()), "value": { "issue_id": issue } },
            { "op": "replace", "path": format!("/workspaces/{issue}"), "value": {} },
            { "op": "replace", "path": "/issues", "value": {} },
        ]))
        .unwrap();
        assert_eq!(
            signals(&patch),
            vec![
                IndexSignal::Issue(issue),
                IndexSignal::Issue(issue),
                IndexSignal::CommentRemoved(comment),
                IndexSignal::Issue(issue),
            ]
        );
    }
}
