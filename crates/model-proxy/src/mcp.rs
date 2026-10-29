//! KanbAIn tools for sandboxed runs (E22), served as a minimal MCP server
//! (JSON-RPC over streamable HTTP, no server-sent stream) at `/mcp` on the
//! model proxy. The run's proxy token authenticates the call and names the
//! run, so tools are scoped to the run's project and ticket.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Inner, client_token};

/// Tools offered to runs. Implemented by the backend.
#[async_trait]
pub trait RunTools: Send + Sync {
    /// MCP tool definitions (`name`, `description`, `inputSchema`).
    fn definitions(&self) -> Vec<Value>;
    /// Runs a tool for a run: the text result, or an error message the
    /// agent sees.
    async fn call(&self, run_id: Uuid, name: &str, arguments: Value) -> Result<String, String>;
}

const PROTOCOL_VERSION: &str = "2025-03-26";

fn reply(id: Value, result: Value) -> Response {
    axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    axum::Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .into_response()
}

pub(crate) async fn mcp_handler(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let grant = client_token(&headers).and_then(|t| inner.grants.read().unwrap().get(&t).cloned());
    let Some(grant) = grant.filter(|g| !g.revoked.load(std::sync::atomic::Ordering::SeqCst)) else {
        return (
            StatusCode::UNAUTHORIZED,
            "unknown or revoked KanbAIn run token",
        )
            .into_response();
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(Value::Null, -32700, "parse error");
    };
    if message.is_array() {
        return rpc_error(Value::Null, -32600, "batches are not supported");
    }
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = message.get("id").cloned() else {
        // A notification (e.g. notifications/initialized).
        return StatusCode::ACCEPTED.into_response();
    };
    let tools = inner.tools.read().unwrap().clone();
    match method {
        "initialize" => {
            let version = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            reply(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "kanbain", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "KanbAIn's search over this project's tickets, past runs and code. \
                                     Search before exploring files, then read only the returned line ranges."
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => {
            let definitions = tools.map(|t| t.definitions()).unwrap_or_default();
            reply(id, json!({ "tools": definitions }))
        }
        "tools/call" => {
            let Some(tools) = tools else {
                return rpc_error(id, -32601, "no tools are available");
            };
            let name = message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let arguments = message
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (text, is_error) = match tools.call(grant.run_id, name, arguments).await {
                Ok(text) => (text, false),
                Err(error) => (error, true),
            };
            reply(
                id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            )
        }
        _ => rpc_error(id, -32601, &format!("method {method} not found")),
    }
}

/// The server-sent event stream is not offered.
pub(crate) async fn mcp_get() -> Response {
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}
