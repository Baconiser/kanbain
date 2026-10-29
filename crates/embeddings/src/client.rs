//! Embedder and reranker clients (E6, E7, E10), called from the backend on
//! the host (E11): batching, prefixes, retries and the dimension probe.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ts_rs::TS;

use crate::config::{EmbedderConfig, EmbedderProvider, RerankerConfig, RerankerProvider};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{url} answered {status}: {body}")]
    Status {
        url: String,
        status: u16,
        body: String,
    },
    #[error("{url} could not be reached: {message}")]
    Transport { url: String, message: String },
    #[error("unexpected answer from {url}: {message}")]
    Decode { url: String, message: String },
    #[error("the embedder returned vectors of dimension {got}, expected {expected}")]
    Dimension { expected: usize, got: usize },
    #[error("{0}")]
    Config(String),
}

impl ClientError {
    /// Worth trying again: rate limits, server errors and network errors.
    fn retryable(&self) -> bool {
        match self {
            Self::Status { status, .. } => *status == 429 || *status >= 500,
            Self::Transport { .. } => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Query,
    Document,
}

#[derive(Debug, Clone, Default)]
pub struct EmbedOutput {
    pub vectors: Vec<Vec<f32>>,
    /// Tokens the provider reports (OpenAI-compatible APIs do).
    pub tokens: Option<u64>,
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String], kind: InputKind) -> Result<EmbedOutput, ClientError>;
}

#[async_trait]
pub trait Reranker: Send + Sync {
    /// Scores for `documents` against `query`, as (index, score), best first.
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<(usize, f32)>, ClientError>;
}

const RETRIES: u32 = 3;

fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim().trim_end_matches('/'), path)
}

async fn post_json(
    http: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    body: &Value,
    timeout: Duration,
    attempts: u32,
) -> Result<Value, ClientError> {
    let mut attempt = 0;
    loop {
        let mut request = http.post(url).timeout(timeout).json(body);
        if let Some(key) = api_key.filter(|k| !k.is_empty()) {
            request = request.bearer_auth(key);
        }
        let result = match request.send().await {
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                if status.is_success() {
                    serde_json::from_str(&text).map_err(|e| ClientError::Decode {
                        url: url.to_string(),
                        message: e.to_string(),
                    })
                } else {
                    Err(ClientError::Status {
                        url: url.to_string(),
                        status: status.as_u16(),
                        body: text.chars().take(300).collect(),
                    })
                }
            }
            Err(e) => Err(ClientError::Transport {
                url: url.to_string(),
                message: e.to_string(),
            }),
        };
        match result {
            Err(e) if e.retryable() && attempt + 1 < attempts => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt - 1))).await;
            }
            other => return other,
        }
    }
}

/// An embedder reached over HTTP.
pub struct HttpEmbedder {
    config: EmbedderConfig,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl HttpEmbedder {
    pub fn new(config: EmbedderConfig, api_key: Option<String>) -> Self {
        Self {
            config,
            api_key,
            http: reqwest::Client::new(),
        }
    }

    pub fn config(&self) -> &EmbedderConfig {
        &self.config
    }

    /// Cuts an input to about `max_input_tokens` (four characters a token).
    fn cap(&self, text: &str) -> String {
        let max_chars = (self.config.max_input_tokens.max(16) as usize) * 4;
        if text.len() <= max_chars {
            return text.to_string();
        }
        let mut cut = max_chars;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text[..cut].to_string()
    }

    async fn embed_batch(&self, inputs: &[String]) -> Result<EmbedOutput, ClientError> {
        let timeout = Duration::from_secs(self.config.timeout_secs.max(1) as u64);
        match self.config.provider {
            EmbedderProvider::Openai => {
                let url = endpoint(&self.config.base_url, "embeddings");
                let mut body = json!({ "model": self.config.model, "input": inputs });
                if let Some(dimensions) = self.config.dimensions {
                    body["dimensions"] = json!(dimensions);
                }
                let value = post_json(
                    &self.http,
                    &url,
                    self.api_key.as_deref(),
                    &body,
                    timeout,
                    RETRIES,
                )
                .await?;
                let decode = |message: &str| ClientError::Decode {
                    url: url.clone(),
                    message: message.to_string(),
                };
                let mut data: Vec<(usize, Vec<f32>)> = value
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or_else(|| decode("no `data` array"))?
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let index = item
                            .get("index")
                            .and_then(Value::as_u64)
                            .map(|n| n as usize)
                            .unwrap_or(i);
                        let vector = item
                            .get("embedding")
                            .and_then(Value::as_array)
                            .ok_or_else(|| decode("an item has no `embedding`"))?
                            .iter()
                            .map(|n| n.as_f64().map(|f| f as f32))
                            .collect::<Option<Vec<f32>>>()
                            .ok_or_else(|| decode("an embedding is not numeric"))?;
                        Ok((index, vector))
                    })
                    .collect::<Result<_, ClientError>>()?;
                data.sort_by_key(|(i, _)| *i);
                let tokens = value
                    .pointer("/usage/total_tokens")
                    .or_else(|| value.pointer("/usage/prompt_tokens"))
                    .and_then(Value::as_u64);
                Ok(EmbedOutput {
                    vectors: data.into_iter().map(|(_, v)| v).collect(),
                    tokens,
                })
            }
            EmbedderProvider::Tei => {
                let url = endpoint(&self.config.base_url, "embed");
                let body = json!({ "inputs": inputs, "truncate": true });
                let value = post_json(
                    &self.http,
                    &url,
                    self.api_key.as_deref(),
                    &body,
                    timeout,
                    RETRIES,
                )
                .await?;
                let vectors: Vec<Vec<f32>> =
                    serde_json::from_value(value).map_err(|e| ClientError::Decode {
                        url,
                        message: e.to_string(),
                    })?;
                Ok(EmbedOutput {
                    vectors,
                    tokens: None,
                })
            }
        }
    }

    /// Embeds a short string and returns the vector's dimension (E7).
    pub async fn probe(&self) -> Result<usize, ClientError> {
        let out = self
            .embed(&["dimension probe".to_string()], InputKind::Query)
            .await?;
        out.vectors
            .first()
            .map(Vec::len)
            .filter(|d| *d > 0)
            .ok_or_else(|| ClientError::Config("the embedder returned no vector".into()))
    }
}

#[async_trait]
impl Embedder for HttpEmbedder {
    async fn embed(&self, texts: &[String], kind: InputKind) -> Result<EmbedOutput, ClientError> {
        if self.config.base_url.trim().is_empty() {
            return Err(ClientError::Config("No embedder URL is set".into()));
        }
        let prefix = match kind {
            InputKind::Query => &self.config.query_prefix,
            InputKind::Document => &self.config.document_prefix,
        };
        let inputs: Vec<String> = texts
            .iter()
            .map(|t| self.cap(&format!("{prefix}{t}")))
            .collect();
        let mut out = EmbedOutput::default();
        for batch in inputs.chunks(self.config.batch_size.max(1) as usize) {
            let part = self.embed_batch(batch).await?;
            if part.vectors.len() != batch.len() {
                return Err(ClientError::Decode {
                    url: self.config.base_url.clone(),
                    message: format!("{} vectors for {} inputs", part.vectors.len(), batch.len()),
                });
            }
            out.vectors.extend(part.vectors);
            if let Some(tokens) = part.tokens {
                out.tokens = Some(out.tokens.unwrap_or(0) + tokens);
            }
        }
        if let Some(first) = out.vectors.first() {
            let dim = first.len();
            if let Some(bad) = out.vectors.iter().find(|v| v.len() != dim) {
                return Err(ClientError::Dimension {
                    expected: dim,
                    got: bad.len(),
                });
            }
        }
        Ok(out)
    }
}

/// A reranker reached over HTTP.
pub struct HttpReranker {
    config: RerankerConfig,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl HttpReranker {
    pub fn new(config: RerankerConfig, api_key: Option<String>) -> Self {
        Self {
            config,
            api_key,
            http: reqwest::Client::new(),
        }
    }
}

/// Documents per rerank request; TEI's default maximum batch is 32.
const RERANK_BATCH: usize = 32;

impl HttpReranker {
    async fn rerank_batch(
        &self,
        query: &str,
        documents: &[String],
        timeout: Duration,
    ) -> Result<Vec<(usize, f32)>, ClientError> {
        let url = endpoint(&self.config.base_url, "rerank");
        let (body, list_key, score_key) = match self.config.provider {
            RerankerProvider::Tei => (
                json!({ "query": query, "texts": documents, "truncate": true }),
                None,
                "score",
            ),
            RerankerProvider::Jina => (
                json!({
                    "model": self.config.model,
                    "query": query,
                    "documents": documents,
                    "top_n": documents.len(),
                }),
                Some("results"),
                "relevance_score",
            ),
        };
        // One shot: a search waiting on a slow reranker is better off with
        // the fused order now, and a retry would only queue more work on a
        // reranker that's already behind.
        let value = post_json(&self.http, &url, self.api_key.as_deref(), &body, timeout, 1).await?;
        let items = match list_key {
            Some(key) => value.get(key).cloned().unwrap_or(Value::Null),
            None => value,
        };
        Ok(items
            .as_array()
            .ok_or_else(|| ClientError::Decode {
                url: url.clone(),
                message: "no result list".into(),
            })?
            .iter()
            .filter_map(|item| {
                let index = item.get("index")?.as_u64()? as usize;
                let score = item.get(score_key)?.as_f64()? as f32;
                (index < documents.len()).then_some((index, score))
            })
            .collect())
    }
}

#[async_trait]
impl Reranker for HttpReranker {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<(usize, f32)>, ClientError> {
        // The timeout covers the whole rerank, however many batches it takes.
        let deadline = Instant::now() + Duration::from_secs(self.config.timeout_secs.max(1) as u64);
        let url = endpoint(&self.config.base_url, "rerank");
        let mut scores = Vec::with_capacity(documents.len());
        for (b, batch) in documents.chunks(RERANK_BATCH).enumerate() {
            let offset = b * RERANK_BATCH;
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClientError::Transport {
                    url,
                    message: "timed out".into(),
                });
            }
            scores.extend(
                self.rerank_batch(query, batch, left)
                    .await?
                    .into_iter()
                    .map(|(i, score)| (i + offset, score)),
            );
        }
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(scores)
    }
}

/// What "Test connection" reports (E7).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConnectionTest {
    pub ok: bool,
    pub model: String,
    /// Vector dimension (embedders only).
    pub dimension: Option<u32>,
    #[ts(type = "number")]
    pub latency_ms: u64,
    /// The URL leaves this machine and its private networks (E9).
    pub remote: bool,
    pub error: Option<String>,
}

pub async fn test_embedder(config: &EmbedderConfig, api_key: Option<String>) -> ConnectionTest {
    let started = Instant::now();
    let embedder = HttpEmbedder::new(config.clone(), api_key);
    let result = embedder.probe().await;
    ConnectionTest {
        ok: result.is_ok(),
        model: config.model.clone(),
        dimension: result.as_ref().ok().map(|d| *d as u32),
        latency_ms: started.elapsed().as_millis() as u64,
        remote: crate::config::is_remote_url(&config.base_url),
        error: result.err().map(|e| e.to_string()),
    }
}

pub async fn test_reranker(config: &RerankerConfig, api_key: Option<String>) -> ConnectionTest {
    let started = Instant::now();
    let reranker = HttpReranker::new(config.clone(), api_key);
    let result = reranker
        .rerank(
            "reset password",
            &[
                "How to reset a password".to_string(),
                "Holiday plans".to_string(),
            ],
        )
        .await;
    let error = match &result {
        Ok(scores) if scores.is_empty() => Some("The reranker returned no scores".to_string()),
        Ok(_) => None,
        Err(e) => Some(e.to_string()),
    };
    ConnectionTest {
        ok: error.is_none(),
        model: config.model.clone(),
        dimension: None,
        latency_ms: started.elapsed().as_millis() as u64,
        remote: crate::config::is_remote_url(&config.base_url),
        error,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};

    use super::*;

    /// A fake embedder: each text becomes a fixed vector derived from its
    /// words, so similar texts get similar vectors.
    pub fn fake_vector(text: &str, dim: usize) -> Vec<f32> {
        let mut v = vec![0f32; dim];
        for word in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
        {
            let mut h: u64 = 1469598103934665603;
            for b in word.bytes() {
                h = (h ^ b as u64).wrapping_mul(1099511628211);
            }
            v[(h % dim as u64) as usize] += 1.0;
        }
        v
    }

    #[derive(Clone, Default)]
    pub struct MockState {
        pub inputs: Arc<Mutex<Vec<Vec<String>>>>,
        pub failures_left: Arc<AtomicUsize>,
        pub dim: usize,
    }

    pub async fn mock_server(state: MockState) -> String {
        async fn openai(
            State(s): State<MockState>,
            Json(body): Json<Value>,
        ) -> Result<Json<Value>, StatusCode> {
            if s.failures_left.load(Ordering::SeqCst) > 0 {
                s.failures_left.fetch_sub(1, Ordering::SeqCst);
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
            let inputs: Vec<String> = serde_json::from_value(body["input"].clone()).unwrap();
            s.inputs.lock().unwrap().push(inputs.clone());
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .rev()
                .map(|(i, t)| json!({ "index": i, "embedding": fake_vector(t, s.dim) }))
                .collect();
            Ok(Json(
                json!({ "data": data, "usage": { "total_tokens": inputs.len() * 3 } }),
            ))
        }
        async fn tei(State(s): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
            let inputs: Vec<String> = serde_json::from_value(body["inputs"].clone()).unwrap();
            s.inputs.lock().unwrap().push(inputs.clone());
            Json(json!(
                inputs
                    .iter()
                    .map(|t| fake_vector(t, s.dim))
                    .collect::<Vec<_>>()
            ))
        }
        async fn tei_rerank(Json(body): Json<Value>) -> Json<Value> {
            let texts: Vec<String> = serde_json::from_value(body["texts"].clone()).unwrap();
            let query = body["query"].as_str().unwrap().to_lowercase();
            Json(json!(
                texts
                    .iter()
                    .enumerate()
                    .map(|(i, t)| json!({ "index": i, "score": if t.to_lowercase().contains(&query) { 0.9 } else { 0.1 } }))
                    .collect::<Vec<_>>()
            ))
        }
        async fn jina_rerank(Json(body): Json<Value>) -> Json<Value> {
            let docs: Vec<String> = serde_json::from_value(body["documents"].clone()).unwrap();
            Json(
                json!({ "results": docs.iter().enumerate().map(|(i, _)| json!({
                "index": i, "relevance_score": i as f64 })).collect::<Vec<_>>() }),
            )
        }
        let app = Router::new()
            .route("/v1/embeddings", post(openai))
            .route("/embed", post(tei))
            .route("/rerank", post(tei_rerank))
            .route("/v1/rerank", post(jina_rerank))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn config(provider: EmbedderProvider, base: String) -> EmbedderConfig {
        EmbedderConfig {
            provider,
            base_url: base,
            model: "mock".into(),
            query_prefix: "search_query: ".into(),
            document_prefix: "search_document: ".into(),
            batch_size: 2,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn openai_batches_prefixes_orders_and_retries() {
        let state = MockState {
            dim: 16,
            failures_left: Arc::new(AtomicUsize::new(1)),
            ..Default::default()
        };
        let base = mock_server(state.clone()).await;
        let embedder =
            HttpEmbedder::new(config(EmbedderProvider::Openai, format!("{base}/v1")), None);
        let texts: Vec<String> = ["alpha beta", "gamma delta", "epsilon zeta"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = embedder.embed(&texts, InputKind::Document).await.unwrap();
        assert_eq!(out.vectors.len(), 3);
        // Answers come back in reverse; they are put in input order.
        assert_eq!(
            out.vectors[0],
            fake_vector("search_document: alpha beta", 16)
        );
        assert_eq!(
            out.vectors[2],
            fake_vector("search_document: epsilon zeta", 16)
        );
        assert_eq!(out.tokens, Some(9));
        let inputs = state.inputs.lock().unwrap().clone();
        assert_eq!(
            inputs.len(),
            2,
            "two batches of at most 2 (the 429 was retried)"
        );
        assert_eq!(inputs[0][0], "search_document: alpha beta");
        assert_eq!(embedder.probe().await.unwrap(), 16);
    }

    #[tokio::test]
    async fn tei_and_rerankers() {
        let state = MockState {
            dim: 8,
            ..Default::default()
        };
        let base = mock_server(state.clone()).await;
        let embedder = HttpEmbedder::new(config(EmbedderProvider::Tei, base.clone()), None);
        let out = embedder
            .embed(&["reset password".into()], InputKind::Query)
            .await
            .unwrap();
        assert_eq!(
            out.vectors[0],
            fake_vector("search_query: reset password", 8)
        );
        assert_eq!(out.tokens, None);

        let tei = HttpReranker::new(
            RerankerConfig {
                provider: RerankerProvider::Tei,
                base_url: base.clone(),
                ..Default::default()
            },
            None,
        );
        let scores = tei
            .rerank("reset", &["holidays".into(), "Reset mail".into()])
            .await
            .unwrap();
        assert_eq!(scores[0].0, 1);

        let jina = HttpReranker::new(
            RerankerConfig {
                provider: RerankerProvider::Jina,
                base_url: format!("{base}/v1"),
                model: "m".into(),
                ..Default::default()
            },
            None,
        );
        let scores = jina
            .rerank("q", &["a".into(), "b".into(), "c".into()])
            .await
            .unwrap();
        assert_eq!(
            scores.iter().map(|s| s.0).collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
    }

    #[tokio::test]
    async fn rerank_batches_once_within_one_deadline() {
        let base = mock_server(MockState::default()).await;
        let tei = |base_url: String, timeout_secs| {
            HttpReranker::new(
                RerankerConfig {
                    provider: RerankerProvider::Tei,
                    base_url,
                    timeout_secs,
                    ..Default::default()
                },
                None,
            )
        };
        // 40 documents take two batches; offsets survive the split.
        let mut docs: Vec<String> = (0..40).map(|i| format!("doc {i}")).collect();
        docs[35] = "the needle".into();
        let scores = tei(base, 5).rerank("needle", &docs).await.unwrap();
        assert_eq!(scores.len(), 40);
        assert_eq!(scores[0].0, 35);

        // A reranker slower than the timeout: one request, no retry, and
        // the second batch is never sent.
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/rerank",
                post(|State(calls): State<Arc<AtomicUsize>>| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    Json(json!([]))
                }),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let started = Instant::now();
        let result = tei(format!("http://{addr}"), 1).rerank("q", &docs).await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(1400));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn connection_test_reports_errors() {
        let test = test_embedder(
            &config(EmbedderProvider::Openai, "http://127.0.0.1:9/v1".into()),
            None,
        )
        .await;
        assert!(!test.ok);
        assert!(test.error.unwrap().contains("could not be reached"));
        assert!(!test.remote);
    }
}
