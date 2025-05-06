mod api;
mod config;
mod firmware;
mod registry;

use anyhow::Result;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::router::create_router;
use crate::config::AppConfig;
use crate::firmware::manager::FirmwareManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(LevelFilter::INFO)
        .with(fmt::layer())
        .init();

    // Load configuration
    let config = AppConfig::from_env()?;
    info!("Configuration loaded");

    // Create firmware manager
    let firmware_manager = Arc::new(FirmwareManager::new(&config));

    info!("Firmware manager created. Server will fetch firmware on demand per device.");

    // Create and start HTTP server
    let router = create_router(firmware_manager);
    let listener = TcpListener::bind(&config.listen_addr).await?;
    info!("OtaFlux listening on {}", config.listen_addr);

    axum::serve(listener, router).await?;
    Ok(())
}
