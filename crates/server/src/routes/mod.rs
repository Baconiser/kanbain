use axum::{
    Router,
    routing::{IntoMakeService, get},
};
use tower_http::{compression::CompressionLayer, validate_request::ValidateRequestHeaderLayer};

use crate::{DeploymentImpl, middleware};

pub mod agents;
pub mod approvals;
pub mod attachments;
pub mod config;
pub mod containers;
pub mod events;
pub mod execution_processes;
pub mod export;
pub mod file_search;
pub mod filesystem;
pub mod frontend;
pub mod health;
pub mod history;
pub mod intake;
pub mod issues;
pub mod plans;
pub mod preview;
pub mod projects;
pub mod repo;
pub mod sandbox;
pub mod scratch;
pub mod search;
pub mod sessions;
pub mod tags;
pub mod terminal;
pub mod workspaces;

pub fn router(deployment: DeploymentImpl) -> IntoMakeService<Router> {
    let api_routes = Router::new()
        .route("/health", get(health::health_check))
        .route("/auth/session", get(middleware::auth_session))
        .merge(agents::router())
        .merge(config::router())
        .merge(containers::router(&deployment))
        .merge(workspaces::router(&deployment))
        .merge(execution_processes::router(&deployment))
        .merge(tags::router(&deployment))
        .merge(projects::router())
        .merge(issues::router())
        .merge(intake::router())
        .merge(history::router())
        .merge(plans::router())
        .merge(search::router())
        .merge(export::router())
        .merge(filesystem::router())
        .merge(repo::router())
        .merge(sandbox::router(&deployment))
        .merge(events::router(&deployment))
        .merge(approvals::router())
        .merge(scratch::router(&deployment))
        .merge(file_search::router(&deployment))
        .merge(preview::api_router())
        .merge(sessions::router(&deployment))
        .merge(terminal::router())
        .nest("/attachments", attachments::routes())
        .layer(axum::middleware::from_fn(middleware::require_launch_token))
        .layer(ValidateRequestHeaderLayer::custom(
            middleware::validate_origin,
        ))
        .layer(axum::middleware::from_fn(middleware::log_server_errors))
        .with_state(deployment);

    Router::new()
        .route("/", get(frontend::serve_frontend_root))
        .route("/{*path}", get(frontend::serve_frontend))
        .nest("/api", api_routes)
        .layer(CompressionLayer::new())
        .into_make_service()
}
