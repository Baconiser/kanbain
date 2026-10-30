//! The app's own warnings and errors (Settings → Logs), so a failure can be
//! read and copied without a console.

use axum::{Router, extract::Query, response::Json as ResponseJson, routing::get};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utils::{
    app_log::{AppLogEntry, app_log},
    response::ApiResponse,
};

use crate::DeploymentImpl;

#[derive(Debug, Deserialize)]
pub struct AppLogQuery {
    /// Only entries after this `seq`.
    #[serde(default)]
    since: u64,
    /// Include warnings, not only errors.
    #[serde(default)]
    warnings: bool,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize, TS)]
pub struct AppLogView {
    pub entries: Vec<AppLogEntry>,
    /// Where the full log is written, when the file could be opened.
    pub file_path: Option<String>,
}

async fn get_logs(Query(query): Query<AppLogQuery>) -> ResponseJson<ApiResponse<AppLogView>> {
    let log = app_log();
    ResponseJson(ApiResponse::success(AppLogView {
        entries: log.entries(
            query.since,
            query.warnings,
            query.limit.unwrap_or(500).min(2000),
        ),
        file_path: log.file_path().map(|p| p.to_string_lossy().into_owned()),
    }))
}

async fn clear_logs() -> ResponseJson<ApiResponse<()>> {
    app_log().clear();
    ResponseJson(ApiResponse::success(()))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new().route("/logs", get(get_logs).delete(clear_logs))
}
