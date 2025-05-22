use axum::{middleware, routing::get, Router};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::api::endpoints::{firmware_handler, health_handler, version_handler};
use crate::firmware_manager::FirmwareManager;
use crate::metrics::middleware::track_metrics;

// Creates the API router with all the necessary routes and middleware.
pub fn api_router(firmware_manager: Arc<FirmwareManager>) -> Router {
    Router::new()
        .route("/version", get(version_handler))
        .route("/firmware", get(firmware_handler))
        .route("/health", get(health_handler))
        .with_state(firmware_manager)
        .route_layer(middleware::from_fn(track_metrics))
        .layer(TraceLayer::new_for_http())
}
