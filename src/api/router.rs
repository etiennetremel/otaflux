use axum::{
    extract::FromRef,
    middleware,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::api::endpoints::{firmware_handler, health_handler, version_handler};
use crate::api::webhooks::harbor::harbor_webhook_handler;
use crate::firmware_manager::FirmwareManager;
use crate::metrics::middleware::track_metrics;
use crate::notifier::Notifier;

// Creates the API router with all the necessary routes and middleware.
#[derive(Clone)]
pub struct AppState {
    pub firmware_manager: Arc<FirmwareManager>,
    pub notifier: Option<Notifier>,
}

impl FromRef<AppState> for Arc<FirmwareManager> {
    fn from_ref(app_state: &AppState) -> Self {
        app_state.firmware_manager.clone()
    }
}

pub fn api_router(firmware_manager: Arc<FirmwareManager>, notifier: Option<Notifier>) -> Router {
    let app_state = AppState {
        firmware_manager,
        notifier,
    };

    let metrics_route = Router::new()
        .route("/version", get(version_handler))
        .route("/firmware", get(firmware_handler))
        .route("/health", get(health_handler))
        .route_layer(middleware::from_fn(track_metrics));

    Router::new()
        .merge(metrics_route)
        .route("/webhooks/harbor", post(harbor_webhook_handler))
        .with_state(app_state)
        .layer(TraceLayer::new_for_http())
}
