//! Per-launch API token (plan D3).
//!
//! The backend generates a random token on every start. Only the app's own
//! webview receives it (as a cookie, via the auth session route), so other
//! local programs, browser pages and sandbox containers cannot call the API.
//! Local developer tooling (the Vite dev proxy, the MCP server) reads it from
//! `<data dir>/launch-token`.

use std::{path::PathBuf, sync::OnceLock};

use crate::assets::asset_dir;

/// Request header carrying the token.
pub const HEADER: &str = "x-kanbain-token";
/// Cookie carrying the token for the webview.
pub const COOKIE: &str = "kanbain_token";
/// Environment variable that fixes the token (tests and scripted setups).
pub const ENV: &str = "KANBAIN_LAUNCH_TOKEN";

static TOKEN: OnceLock<String> = OnceLock::new();

/// The token for this process: `KANBAIN_LAUNCH_TOKEN` if set, otherwise
/// 256 random bits generated on first use.
pub fn launch_token() -> &'static str {
    TOKEN.get_or_init(|| {
        std::env::var(ENV)
            .ok()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                )
            })
    })
}

pub fn token_file_path() -> PathBuf {
    asset_dir().join("launch-token")
}

/// Writes the token file, readable only by the current user on Unix.
pub fn write_token_file() -> std::io::Result<()> {
    let path = token_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(launch_token().as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, launch_token())
    }
}

/// The token of a running backend: `KANBAIN_LAUNCH_TOKEN`, else the token file.
pub fn read_token() -> Option<String> {
    std::env::var(ENV)
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| std::fs::read_to_string(token_file_path()).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Constant-time comparison against this process's token.
pub fn matches(candidate: &str) -> bool {
    let expected = launch_token().as_bytes();
    let candidate = candidate.as_bytes();
    if expected.len() != candidate.len() {
        return false;
    }
    expected
        .iter()
        .zip(candidate)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_long_and_matches_itself_only() {
        let token = launch_token();
        assert!(token.len() >= 32);
        assert!(matches(token));
        assert!(!matches(""));
        assert!(!matches(&token[1..]));
        let mut wrong = token.to_string();
        wrong.replace_range(0..1, if token.starts_with('a') { "b" } else { "a" });
        assert!(!matches(&wrong));
    }
}
