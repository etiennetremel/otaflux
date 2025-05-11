mod api;
mod config;
mod firmware;
mod metrics;
mod registry;

use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::router::api_router;
use crate::config::AppConfig;
use crate::firmware::manager::FirmwareManager;
use crate::metrics::router::metrics_router;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(fmt::layer())
        .init();

    // Load configuration
    let config = AppConfig::from_env().unwrap();
    info!("Configuration loaded");

    // Create firmware manager
    let firmware_manager = Arc::new(FirmwareManager::new(&config));

    info!("Firmware manager created. Server will fetch firmware on demand per device.");

    let (_main_server, _metrics_server) = tokio::join!(
        start_main_server(&config.listen_addr, firmware_manager),
        start_metrics_server(&config.metrics_listen_addr)
    );
}

async fn start_main_server(listen_address: &str, firmware_manager: Arc<FirmwareManager>) {
    let listener = TcpListener::bind(listen_address).await.unwrap();
    tracing::debug!("OtaFlux listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, api_router(firmware_manager))
        .await
        .unwrap();
}

async fn start_metrics_server(listen_address: &str) {
    let listener = TcpListener::bind(listen_address).await.unwrap();
    tracing::debug!(
        "metrics server listening on {}",
        listener.local_addr().unwrap()
    );
    axum::serve(listener, metrics_router()).await.unwrap();
}
