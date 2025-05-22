mod api;
mod firmware_manager;
mod metrics;
mod registry;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::router::api_router;
use crate::firmware_manager::FirmwareManager;
use crate::metrics::router::metrics_router;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {
    #[clap(long, env)]
    pub registry_url: String,
    #[clap(long, env, value_parser = normalize_repository_prefix)]
    pub repository_prefix: String,
    #[clap(long, env)]
    pub registry_username: String,
    #[clap(long, env)]
    pub registry_password: String,
    #[clap(long, env, required(false), default_value_t = false)]
    pub registry_insecure: bool,
    #[clap(long, env, required(false))]
    pub cosign_pub_key_path: Option<String>,
    #[clap(long, env, default_value = "0.0.0.0:8080")]
    pub listen_addr: String,
    #[clap(long, env, default_value = "0.0.0.0:9090")]
    pub metrics_listen_addr: String,
    #[clap(long, env, default_value = "info")]
    log_level: LevelFilter,
}

fn normalize_repository_prefix(val: &str) -> Result<String, String> {
    let trimmed = val.strip_suffix('/').unwrap_or(val);
    Ok(trimmed.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(cli.log_level)
        .with(fmt::layer())
        .init();

    // Graceful shutdown setup
    let cancel_token = CancellationToken::new();

    let ctrl_c_listener_task = tokio::spawn({
        let cancel_token_clone = cancel_token.clone();
        async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for Ctrl+C signal");
            info!("Ctrl+C received, proceeding with graceful shutdown...");
            cancel_token_clone.cancel();
        }
    });

    // Firmware manager initialization
    let firmware_manager = Arc::new(
        FirmwareManager::new(
            cli.registry_url,
            cli.registry_username,
            cli.registry_password,
            cli.registry_insecure,
            cli.repository_prefix,
            cli.cosign_pub_key_path,
        )
        .map_err(|e| {
            error!("Failed to initialize FirmwareManager: {:?}", e);
            e
        })?,
    );

    info!("Firmware manager created. Server will fetch firmware on demand per device.");

    // Start servers
    let fm = Arc::clone(&firmware_manager);

    let main_server_cancel_token = cancel_token.clone();
    let metrics_server_cancel_token = cancel_token.clone();

    tokio::try_join!(
        start_main_server(&cli.listen_addr, fm, main_server_cancel_token),
        start_metrics_server(&cli.metrics_listen_addr, metrics_server_cancel_token)
    )?;

    // Waits for signal before exiting gracefully
    ctrl_c_listener_task.await?;

    info!("All services shut down gracefully.");

    Ok(())
}

async fn start_main_server(
    listen_address: &str,
    firmware_manager: Arc<FirmwareManager>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(listen_address).await?;
    info!("OtaFlux listening on {}", listener.local_addr()?);

    let shutdown_future = async move {
        cancel_token.cancelled().await;
    };

    axum::serve(listener, api_router(firmware_manager))
        .with_graceful_shutdown(shutdown_future) // Pass the 'static future
        .await?;
    info!("Main server shut down gracefully");
    Ok(())
}

async fn start_metrics_server(listen_address: &str, cancel_token: CancellationToken) -> Result<()> {
    let listener = TcpListener::bind(listen_address).await?;
    info!("Metrics server listening on {}", listener.local_addr()?);

    let shutdown_future = async move {
        cancel_token.cancelled().await;
    };

    axum::serve(listener, metrics_router())
        .with_graceful_shutdown(shutdown_future) // Pass the 'static future
        .await?;
    info!("Metrics server shut down gracefully");
    Ok(())
}
