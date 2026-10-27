use axum::{
    Router,
    routing::{IntoMakeService, get},
};
use tower_http::{compression::CompressionLayer, validate_request::ValidateRequestHeaderLayer};

use crate::{DeploymentImpl, middleware};

pub mod approvals;
pub mod attachments;
pub mod config;
pub mod containers;
pub mod events;
pub mod execution_processes;
pub mod filesystem;
pub mod frontend;
pub mod health;
pub mod issues;
pub mod preview;
pub mod projects;
pub mod repo;
pub mod scratch;
pub mod search;
pub mod sessions;
pub mod tags;
pub mod terminal;
pub mod workspaces;

pub fn router(deployment: DeploymentImpl) -> IntoMakeService<Router> {
    let api_routes = Router::new()
        .route("/health", get(health::health_check))
        .merge(config::router())
        .merge(containers::router(&deployment))
        .merge(workspaces::router(&deployment))
        .merge(execution_processes::router(&deployment))
        .merge(tags::router(&deployment))
        .merge(projects::router())
        .merge(issues::router())
        .merge(filesystem::router())
        .merge(repo::router())
        .merge(events::router(&deployment))
        .merge(approvals::router())
        .merge(scratch::router(&deployment))
        .merge(search::router(&deployment))
        .merge(preview::api_router())
        .merge(sessions::router(&deployment))
        .merge(terminal::router())
        .nest("/attachments", attachments::routes())
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
