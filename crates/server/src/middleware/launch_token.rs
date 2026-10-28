//! Rejects API requests without this launch's token (plan D3).

use axum::{
    extract::{Query, Request},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use utils::launch_token;

/// Paths (relative to `/api`) that work without a token.
const PUBLIC_PATHS: &[&str] = &["/health", "/auth/session"];

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == launch_token::COOKIE)
        .map(|(_, value)| value)
}

/// Whether the request carries the token in its header or cookie.
pub fn has_valid_token(headers: &HeaderMap) -> bool {
    headers
        .get(launch_token::HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(launch_token::matches)
        || cookie_token(headers).is_some_and(launch_token::matches)
}

pub async fn require_launch_token(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if PUBLIC_PATHS.contains(&path) || has_valid_token(request.headers()) {
        return next.run(request).await;
    }
    (StatusCode::UNAUTHORIZED, "Missing or invalid launch token").into_response()
}

#[derive(Debug, Deserialize)]
pub struct AuthSessionQuery {
    token: String,
    #[serde(default)]
    next: Option<String>,
}

/// Exchanges the launch token for an HttpOnly session cookie, then redirects
/// into the app. The Tauri shell opens this URL first.
pub async fn auth_session(Query(query): Query<AuthSessionQuery>) -> Response {
    if !launch_token::matches(&query.token) {
        return (StatusCode::UNAUTHORIZED, "Invalid launch token").into_response();
    }
    // Only same-origin relative paths; `//host` would leave the app.
    let next = query
        .next
        .filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.contains('\\'))
        .unwrap_or_else(|| "/".to_string());
    let cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict",
        launch_token::COOKIE,
        launch_token::launch_token()
    );
    ([(header::SET_COOKIE, cookie)], Redirect::to(&next)).into_response()
}

/// URL that signs the webview in, given the server's base URL.
pub fn auth_session_url(base_url: &str) -> String {
    format!(
        "{}/api/auth/session?token={}",
        base_url.trim_end_matches('/'),
        launch_token::launch_token()
    )
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn accepts_header_or_cookie() {
        let token = launch_token::launch_token();
        let mut headers = HeaderMap::new();
        assert!(!has_valid_token(&headers));

        headers.insert(launch_token::HEADER, HeaderValue::from_str(token).unwrap());
        assert!(has_valid_token(&headers));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("theme=dark; {}={token}", launch_token::COOKIE))
                .unwrap(),
        );
        assert!(has_valid_token(&headers));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("kanbain_token=wrong"),
        );
        assert!(!has_valid_token(&headers));
    }
}
