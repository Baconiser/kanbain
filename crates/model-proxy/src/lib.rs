//! Model proxy (D40).
//!
//! Sandbox containers talk to LLM APIs only through this proxy. Each run
//! gets a short-lived token that maps every call to the run and its budget.
//! The proxy swaps the token for the real credential, streams responses
//! through, parses usage (including cache tokens), prices it, reports every
//! call, and cuts off a run that reaches its budget.
//!
//! Routes:
//! - `/v1/*`: Anthropic API (Claude Code with `ANTHROPIC_BASE_URL` = proxy root).
//! - `/openai/v1/*`: an OpenAI-compatible API (`/chat/completions`, `/responses`).
//! - `/p/<id>/*`: a custom OpenAI-compatible provider added in the settings;
//!   the rest of the path is appended to its base URL.

pub mod mcp;
pub mod prices;
pub mod usage;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderName, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::{Stream, StreamExt};
use rand::{Rng, distr::Alphanumeric};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::usage::{SseParser, UsageTracker};
pub use crate::{prices::PriceTable, usage::Usage};

const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
/// Rough characters per output token, used to estimate spend while a
/// response is still streaming (Anthropic reports output tokens at the end).
const CHARS_PER_TOKEN: u64 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    /// A custom OpenAI-compatible provider, by id. Serialized as the id.
    #[serde(untagged)]
    Custom(String),
}

impl Provider {
    /// Speaks the OpenAI API (chat completions, usage chunks).
    fn is_openai_compatible(&self) -> bool {
        matches!(self, Provider::OpenAi | Provider::Custom(_))
    }
}

/// The proxy path prefix for a custom provider.
pub fn custom_provider_path(id: &str) -> String {
    format!("/p/{id}")
}

/// Where to send a provider's requests and which auth headers to add.
#[derive(Debug, Clone)]
pub struct Upstream {
    /// Base URL without a trailing slash, e.g. `https://api.anthropic.com`.
    pub base_url: String,
    pub headers: Vec<(String, String)>,
}

/// Supplies real credentials. They never reach the container.
#[async_trait]
pub trait Credentials: Send + Sync {
    async fn upstream(&self, provider: &Provider) -> Option<Upstream>;
}

/// One proxied model call, reported when its response has finished.
#[derive(Debug, Clone, Serialize)]
pub struct ModelCallRecord {
    pub run_id: Uuid,
    pub provider: Provider,
    pub path: String,
    pub model: Option<String>,
    pub status: u16,
    pub usage: Usage,
    /// `None` when the model has no entry in the price table.
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
    pub streamed: bool,
    /// The proxy ended the response because the budget ran out.
    pub cut_off: bool,
}

/// Receives the proxy's records. Implemented by the backend to persist events
/// and stop runs.
#[async_trait]
pub trait ProxyEvents: Send + Sync {
    async fn model_call(&self, record: ModelCallRecord);
    /// The run reached its budget; its token is already revoked.
    async fn budget_exhausted(&self, run_id: Uuid, spent_usd: f64);
}

#[derive(Debug)]
struct Grant {
    run_id: Uuid,
    budget_usd: Option<f64>,
    spent_usd: Mutex<f64>,
    revoked: AtomicBool,
    exhausted: AtomicBool,
}

impl Grant {
    fn spent(&self) -> f64 {
        *self.spent_usd.lock().unwrap()
    }

    fn over_budget(&self, extra: f64) -> bool {
        self.budget_usd
            .is_some_and(|budget| self.spent() + extra >= budget)
    }
}

struct Inner {
    grants: RwLock<HashMap<String, Arc<Grant>>>,
    tools: RwLock<Option<Arc<dyn mcp::RunTools>>>,
    prices: RwLock<PriceTable>,
    credentials: Arc<dyn Credentials>,
    events: Arc<dyn ProxyEvents>,
    client: reqwest::Client,
}

#[derive(Clone)]
pub struct ModelProxy {
    inner: Arc<Inner>,
}

impl ModelProxy {
    pub fn new(
        prices: PriceTable,
        credentials: Arc<dyn Credentials>,
        events: Arc<dyn ProxyEvents>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                grants: RwLock::new(HashMap::new()),
                tools: RwLock::new(None),
                prices: RwLock::new(prices),
                credentials,
                events,
                client: reqwest::Client::builder()
                    .no_gzip()
                    .no_brotli()
                    .build()
                    .expect("reqwest client"),
            }),
        }
    }

    /// Issue a token for a run. `already_spent_usd` counts against
    /// `budget_usd` (for example a ticket's earlier runs).
    pub fn issue_token(
        &self,
        run_id: Uuid,
        budget_usd: Option<f64>,
        already_spent_usd: f64,
    ) -> String {
        let token: String = format!(
            "kbr-{}",
            rand::rng()
                .sample_iter(&Alphanumeric)
                .take(40)
                .map(char::from)
                .collect::<String>()
        );
        let grant = Arc::new(Grant {
            run_id,
            budget_usd,
            spent_usd: Mutex::new(already_spent_usd),
            revoked: AtomicBool::new(false),
            exhausted: AtomicBool::new(false),
        });
        self.inner
            .grants
            .write()
            .unwrap()
            .insert(token.clone(), grant);
        token
    }

    /// Revoke every token of a run; in-flight streams end at their next chunk.
    pub fn revoke_run(&self, run_id: Uuid) {
        let mut grants = self.inner.grants.write().unwrap();
        grants.retain(|_, grant| {
            if grant.run_id == run_id {
                grant.revoked.store(true, Ordering::SeqCst);
                false
            } else {
                true
            }
        });
    }

    pub fn set_prices(&self, prices: PriceTable) {
        *self.inner.prices.write().unwrap() = prices;
    }

    /// Tools runs reach at `/mcp` (E22).
    pub fn set_tools(&self, tools: Arc<dyn mcp::RunTools>) {
        *self.inner.tools.write().unwrap() = Some(tools);
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route(
                "/mcp",
                axum::routing::post(mcp::mcp_handler).get(mcp::mcp_get),
            )
            .fallback(proxy_handler)
            .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
            .with_state(self.inner.clone())
    }

    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        axum::serve(listener, self.router()).await
    }
}

fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    // Anthropic's error shape; OpenAI clients read the message field too.
    (
        status,
        axum::Json(json!({
            "type": "error",
            "error": {"type": kind, "message": message}
        })),
    )
        .into_response()
}

fn client_token(headers: &HeaderMap) -> Option<String> {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(key.trim().to_string());
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(|t| t.trim().to_string())
}

/// Request headers never forwarded upstream.
fn is_dropped_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "authorization"
            | "x-api-key"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "accept-encoding"
    )
}

fn is_dropped_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-length" | "connection" | "keep-alive" | "transfer-encoding" | "content-encoding"
    )
}

async fn proxy_handler(State(inner): State<Arc<Inner>>, request: Request) -> Response {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());

    let (provider, upstream_path) = if let Some(rest) = path_and_query.strip_prefix("/openai") {
        (Provider::OpenAi, rest.to_string())
    } else if let Some(rest) = path_and_query.strip_prefix("/p/")
        && let Some((id, rest)) = rest.split_once('/')
        && !id.is_empty()
    {
        (Provider::Custom(id.to_string()), format!("/{rest}"))
    } else if path_and_query.starts_with("/v1/") {
        (Provider::Anthropic, path_and_query.clone())
    } else {
        return error_response(StatusCode::NOT_FOUND, "not_found_error", "unknown route");
    };

    let Some(token) = client_token(&parts.headers) else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "missing KanbAIn run token",
        );
    };
    let grant = inner.grants.read().unwrap().get(&token).cloned();
    let Some(grant) = grant.filter(|g| !g.revoked.load(Ordering::SeqCst)) else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "unknown or revoked KanbAIn run token",
        );
    };
    if grant.exhausted.load(Ordering::SeqCst) || grant.over_budget(0.0) {
        return error_response(
            StatusCode::PAYMENT_REQUIRED,
            "budget_exceeded",
            "KanbAIn budget for this run is exhausted",
        );
    }

    let Some(upstream) = inner.credentials.upstream(&provider).await else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "authentication_error",
            "KanbAIn has no credentials configured for this provider",
        );
    };

    let body = match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &e.to_string(),
            );
        }
    };
    let (body, requested_model) = prepare_body(&provider, &upstream_path, body);

    let url = format!(
        "{}{}",
        upstream.base_url.trim_end_matches('/'),
        upstream_path
    );
    let mut builder = inner.client.request(parts.method.clone(), &url);
    // Beta flags the credential needs (OAuth) join the client's own.
    let mut betas: Vec<String> = Vec::new();
    for (name, value) in parts.headers.iter() {
        if name.as_str() == ANTHROPIC_BETA {
            betas.extend(value.to_str().ok().map(str::to_string));
        } else if !is_dropped_request_header(name) {
            builder = builder.header(name, value);
        }
    }
    builder = builder.header(header::ACCEPT_ENCODING, "identity");
    for (name, value) in &upstream.headers {
        if name.eq_ignore_ascii_case(ANTHROPIC_BETA) {
            betas.push(value.clone());
        } else {
            builder = builder.header(name, value);
        }
    }
    if let Some(beta) = merge_betas(&betas) {
        builder = builder.header(ANTHROPIC_BETA, beta);
    }

    let upstream_response = match builder.body(body).send().await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("Model proxy upstream request failed: {e}");
            return error_response(StatusCode::BAD_GATEWAY, "api_error", "upstream unreachable");
        }
    };

    let status = upstream_response.status();
    let mut response_headers = HeaderMap::new();
    for (name, value) in upstream_response.headers() {
        if !is_dropped_response_header(name) {
            response_headers.append(name.clone(), value.clone());
        }
    }
    let streamed = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));

    let call = CallGuard {
        inner: inner.clone(),
        grant,
        record: ModelCallRecord {
            run_id: Uuid::nil(),
            provider: provider.clone(),
            path: parts.uri.path().to_string(),
            model: requested_model.clone(),
            status: status.as_u16(),
            usage: Usage::default(),
            cost_usd: None,
            latency_ms: 0,
            streamed,
            cut_off: false,
        },
        tracker: UsageTracker::with_model(requested_model),
        started,
        // Only metered endpoints produce usage; count_tokens etc. are free.
        metered: is_metered(&provider, parts.uri.path()),
    };

    let body = if streamed {
        Body::from_stream(stream_with_metering(upstream_response.bytes_stream(), call))
    } else {
        let mut call = call;
        match upstream_response.bytes().await {
            Ok(bytes) => {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    call.tracker.observe(&value);
                }
                Body::from(bytes)
            }
            Err(e) => {
                tracing::warn!("Model proxy upstream body failed: {e}");
                return error_response(StatusCode::BAD_GATEWAY, "api_error", "upstream failed");
            }
        }
    };

    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

const ANTHROPIC_BETA: &str = "anthropic-beta";
/// Beta flag that lets a Claude account's OAuth token call the API.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Comma-separated, de-duplicated beta flags, in first-seen order.
fn merge_betas(values: &[String]) -> Option<String> {
    let mut flags: Vec<&str> = Vec::new();
    for flag in values.iter().flat_map(|v| v.split(',')).map(str::trim) {
        if !flag.is_empty() && !flags.contains(&flag) {
            flags.push(flag);
        }
    }
    (!flags.is_empty()).then(|| flags.join(","))
}

fn is_metered(provider: &Provider, path: &str) -> bool {
    if provider.is_openai_compatible() {
        path.ends_with("/chat/completions") || path.ends_with("/responses")
    } else {
        path == "/v1/messages"
    }
}

/// Read the requested model and, for streamed OpenAI chat completions, ask
/// for the final usage chunk.
fn prepare_body(provider: &Provider, path: &str, body: Bytes) -> (Bytes, Option<String>) {
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return (body, None);
    };
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string);
    if provider.is_openai_compatible()
        && path.contains("/chat/completions")
        && value.get("stream").and_then(Value::as_bool) == Some(true)
        && let Some(object) = value.as_object_mut()
    {
        let options = object.entry("stream_options").or_insert_with(|| json!({}));
        if let Some(options) = options.as_object_mut() {
            options.insert("include_usage".into(), Value::Bool(true));
        }
        if let Ok(rewritten) = serde_json::to_vec(&value) {
            return (Bytes::from(rewritten), model);
        }
    }
    (body, model)
}

/// Tracks one call; reports it and charges the run when dropped, which
/// covers normal completion, cut-off and client disconnects alike.
struct CallGuard {
    inner: Arc<Inner>,
    grant: Arc<Grant>,
    record: ModelCallRecord,
    tracker: UsageTracker,
    started: Instant,
    metered: bool,
}

impl CallGuard {
    fn cost(&self, usage: &Usage) -> Option<f64> {
        let model = self.tracker.model.as_deref()?;
        self.inner.prices.read().unwrap().cost(model, usage)
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        let mut record = self.record.clone();
        record.run_id = self.grant.run_id;
        record.model = self.tracker.model.clone();
        record.usage = self.tracker.usage;
        record.latency_ms = self.started.elapsed().as_millis() as u64;
        record.cost_usd = if self.metered {
            self.cost(&record.usage)
        } else {
            Some(0.0)
        };
        let cost = record.cost_usd.unwrap_or(0.0);
        let spent = {
            let mut spent = self.grant.spent_usd.lock().unwrap();
            *spent += cost;
            *spent
        };
        let exhausted =
            self.grant.over_budget(0.0) && !self.grant.exhausted.swap(true, Ordering::SeqCst);
        let events = self.inner.events.clone();
        let run_id = self.grant.run_id;
        let send = async move {
            events.model_call(record).await;
            if exhausted {
                events.budget_exhausted(run_id, spent).await;
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(send);
            }
            Err(_) => tracing::warn!("Model call for run {run_id} dropped outside a runtime"),
        }
    }
}

/// Pass SSE bytes through unchanged while metering them. When the run's
/// committed spend plus this call's running estimate reaches the budget, an
/// error event is sent and the stream ends.
fn stream_with_metering(
    upstream: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    call: CallGuard,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct StreamState<S> {
        upstream: std::pin::Pin<Box<S>>,
        call: CallGuard,
        parser: SseParser,
        streamed_chars: u64,
        done: bool,
    }
    let state = StreamState {
        upstream: Box::pin(upstream),
        call,
        parser: SseParser::default(),
        streamed_chars: 0,
        done: false,
    };
    futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        match state.upstream.next().await {
            None => None,
            Some(Err(e)) => {
                state.done = true;
                Some((Err(std::io::Error::other(e)), state))
            }
            Some(Ok(chunk)) => {
                for data in state.parser.feed(&chunk) {
                    if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        state.call.tracker.observe(&value);
                        state.streamed_chars += delta_chars(&value);
                    }
                }
                let revoked = state.call.grant.revoked.load(Ordering::SeqCst);
                let mut estimate = state.call.tracker.usage;
                estimate.output_tokens = estimate
                    .output_tokens
                    .max(state.streamed_chars / CHARS_PER_TOKEN);
                let running = state.call.cost(&estimate).unwrap_or(0.0);
                if revoked || (state.call.metered && state.call.grant.over_budget(running)) {
                    state.done = true;
                    state.call.record.cut_off = true;
                    // Charge at least the estimate for what was generated.
                    state.call.tracker.usage = estimate;
                    let mut out = chunk.to_vec();
                    out.extend_from_slice(
                        b"\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"budget_exceeded\",\"message\":\"KanbAIn stopped this run: budget exhausted or run cancelled\"}}\n\n",
                    );
                    return Some((Ok(Bytes::from(out)), state));
                }
                Some((Ok(chunk), state))
            }
        }
    })
}

/// Generated characters in an Anthropic content delta or OpenAI chunk.
fn delta_chars(value: &Value) -> u64 {
    let text_len = |v: Option<&Value>| v.and_then(Value::as_str).map_or(0, |s| s.len() as u64);
    if let Some(delta) = value.get("delta") {
        // Anthropic content_block_delta / OpenAI Responses output_text.delta
        if let Some(s) = delta.as_str() {
            return s.len() as u64;
        }
        return text_len(delta.get("text"))
            + text_len(delta.get("partial_json"))
            + text_len(delta.get("thinking"));
    }
    // OpenAI chat chunk
    value
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|c| {
                    let delta = c.get("delta");
                    text_len(delta.and_then(|d| d.get("content")))
                        + text_len(delta.and_then(|d| d.get("reasoning_content")))
                })
                .sum()
        })
        .unwrap_or(0)
}

/// A static API-key credential source.
pub struct StaticCredentials {
    pub anthropic: Option<Upstream>,
    pub openai: Option<Upstream>,
    pub custom: HashMap<String, Upstream>,
}

impl StaticCredentials {
    pub fn anthropic_api_key(base_url: &str, key: &str) -> Upstream {
        Upstream {
            base_url: base_url.to_string(),
            headers: vec![("x-api-key".into(), key.to_string())],
        }
    }

    /// A Claude account (subscription) token from `claude setup-token`.
    pub fn anthropic_oauth(base_url: &str, token: &str) -> Upstream {
        Upstream {
            base_url: base_url.to_string(),
            headers: vec![
                ("authorization".into(), format!("Bearer {token}")),
                (ANTHROPIC_BETA.into(), OAUTH_BETA.into()),
            ],
        }
    }

    pub fn bearer(base_url: &str, key: &str) -> Upstream {
        Upstream {
            base_url: base_url.to_string(),
            headers: vec![("authorization".into(), format!("Bearer {key}"))],
        }
    }
}

#[async_trait]
impl Credentials for StaticCredentials {
    async fn upstream(&self, provider: &Provider) -> Option<Upstream> {
        match provider {
            Provider::Anthropic => self.anthropic.clone(),
            Provider::OpenAi => self.openai.clone(),
            Provider::Custom(id) => self.custom.get(id).cloned(),
        }
    }
}

#[cfg(test)]
mod tests;
