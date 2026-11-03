//! Remote MCP servers from the settings, reached by runs at
//! `/mcp/<name>` on the proxy. The run token picks the run; the proxy
//! forwards to the server's real URL with its real headers, so credentials
//! never enter the container and the server's host needs no egress grant.
//! Bodies and server-sent streams pass through unchanged.

use std::sync::{Arc, atomic::Ordering};

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;

use crate::{Inner, MAX_REQUEST_BYTES, client_token, is_dropped_request_header,
    is_dropped_response_header};

/// The proxy path for a remote MCP server.
pub fn remote_mcp_path(name: &str) -> String {
    format!("/mcp/{name}")
}

pub(crate) async fn root_handler(
    State(inner): State<Arc<Inner>>,
    Path(name): Path<String>,
    request: Request,
) -> Response {
    forward(inner, name, String::new(), request).await
}

pub(crate) async fn rest_handler(
    State(inner): State<Arc<Inner>>,
    Path((name, rest)): Path<(String, String)>,
    request: Request,
) -> Response {
    forward(inner, name, format!("/{rest}"), request).await
}

fn grant_run(inner: &Inner, headers: &HeaderMap) -> Option<uuid::Uuid> {
    let token = client_token(headers)?;
    let grant = inner.grants.read().unwrap().get(&token).cloned()?;
    (!grant.revoked.load(Ordering::SeqCst)).then_some(grant.run_id)
}

async fn forward(inner: Arc<Inner>, name: String, rest: String, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(run_id) = grant_run(&inner, &parts.headers) else {
        return (StatusCode::UNAUTHORIZED, "unknown or revoked KanbAIn run token")
            .into_response();
    };
    let upstream = inner
        .mcp_upstreams
        .read()
        .unwrap()
        .get(&run_id)
        .and_then(|servers| servers.get(&name))
        .cloned();
    let Some(upstream) = upstream else {
        return (StatusCode::NOT_FOUND, "no such MCP server for this run").into_response();
    };
    let body = match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let mut url = format!("{}{rest}", upstream.base_url.trim_end_matches('/'));
    if let Some(query) = parts.uri.query() {
        url.push('?');
        url.push_str(query);
    }
    let mut builder = inner.client.request(parts.method, &url);
    for (header_name, value) in parts.headers.iter() {
        if !is_dropped_request_header(header_name) {
            builder = builder.header(header_name, value);
        }
    }
    builder = builder.header(header::ACCEPT_ENCODING, "identity");
    for (header_name, value) in &upstream.headers {
        builder = builder.header(header_name, value);
    }
    let response = match builder.body(body).send().await {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("MCP server {name} unreachable: {e}");
            return (StatusCode::BAD_GATEWAY, format!("MCP server {name} unreachable"))
                .into_response();
        }
    };

    let mut headers = HeaderMap::new();
    for (header_name, value) in response.headers() {
        if !is_dropped_response_header(header_name) {
            headers.append(header_name.clone(), value.clone());
        }
    }
    let status = response.status();
    let stream = response.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other));
    let mut out = Response::new(Body::from_stream(stream));
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    out
}
