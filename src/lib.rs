pub mod api;
pub mod firmware_manager;
pub mod metrics;
pub mod notifier;
pub mod registry;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::router::api_router;
use crate::firmware_manager::FirmwareManager;
use crate::metrics::router::metrics_router;
use crate::notifier::{Notifier, TlsConfig};

const DEFAULT_CACHE_SIZE: usize = 100;
/// Initial backoff delay for MQTT reconnection attempts (in milliseconds).
const MQTT_INITIAL_BACKOFF_MS: u64 = 100;
/// Maximum backoff delay for MQTT reconnection attempts (in milliseconds).
/// Caps the exponential growth to prevent excessively long waits.
const MQTT_MAX_BACKOFF_MS: u64 = 30_000;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {
    #[clap(long, env)]
    pub registry_url: String,
    #[clap(long, env)]
    pub mqtt_url: Option<String>,
    #[clap(long, env, default_value = "")]
    pub mqtt_username: String,
    #[clap(long, env, default_value = "")]
    pub mqtt_password: String,
    #[clap(long, env, default_value = "")]
    pub mqtt_topic: String,
    /// Path to MQTT CA certificate file (enables TLS if provided)
    #[clap(long, env)]
    pub mqtt_ca_cert_path: Option<String>,
    /// Path to MQTT client certificate file
    #[clap(long, env)]
    pub mqtt_client_cert_path: Option<String>,
    /// Path to MQTT client key file
    #[clap(long, env)]
    pub mqtt_client_key_path: Option<String>,
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
    #[clap(long, env, default_value_t = DEFAULT_CACHE_SIZE)]
    pub cache_size: usize,
}

#[allow(clippy::unnecessary_wraps)]
fn normalize_repository_prefix(val: &str) -> Result<String, String> {
    let trimmed = val.strip_suffix('/').unwrap_or(val);
    Ok(trimmed.to_string())
}

/// Runs the `OtaFlux` server with the provided CLI configuration.
///
/// This function initializes logging, sets up graceful shutdown handling,
/// creates the firmware manager, and starts both the main API server and
/// metrics server. It also optionally initializes an MQTT notifier if
/// configured.
///
/// # Arguments
///
/// * `cli` - The parsed command-line arguments containing server configuration.
///
/// # Errors
///
/// Returns an error if:
/// - The firmware manager fails to initialize.
/// - Reading MQTT TLS certificates fails.
/// - The MQTT notifier fails to initialize.
/// - Binding to the configured listen addresses fails.
///
/// # Panics
///
/// Panics if the Ctrl+C signal handler fails to register.
#[allow(clippy::too_many_lines)]
pub async fn run(cli: Cli) -> Result<()> {
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
    let firmware_manager = Arc::new(FirmwareManager::with_cache_size(
        cli.registry_url,
        cli.registry_username,
        cli.registry_password,
        cli.registry_insecure,
        &cli.repository_prefix,
        cli.cosign_pub_key_path,
        cli.cache_size,
    )?);

    info!(
        cache_size = cli.cache_size,
        "Firmware manager created. Server will fetch firmware on demand per device."
    );

    let fm = Arc::clone(&firmware_manager);

    let main_server_cancel_token = cancel_token.clone();
    let metrics_server_cancel_token = cancel_token.clone();

    // MQTT notifier setup
    let mut notifier: Option<Notifier> = None;
    if let Some(mqtt_url) = cli.mqtt_url {
        // Build TLS config if CA certificate path is provided
        let tls_config = if let Some(ca_path) = &cli.mqtt_ca_cert_path {
            let ca_cert = std::fs::read(ca_path)
                .map_err(|e| anyhow::anyhow!("Failed to read MQTT CA cert: {e}"))?;

            // Client auth is optional - only if both cert and key are provided
            let client_auth = match (&cli.mqtt_client_cert_path, &cli.mqtt_client_key_path) {
                (Some(cert_path), Some(key_path)) => {
                    let client_cert = std::fs::read(cert_path)
                        .map_err(|e| anyhow::anyhow!("Failed to read MQTT client cert: {e}"))?;
                    let client_key = std::fs::read(key_path)
                        .map_err(|e| anyhow::anyhow!("Failed to read MQTT client key: {e}"))?;
                    Some((client_cert, client_key))
                }
                (None, None) => None,
                _ => {
                    warn!(
                        "Incomplete MQTT client auth configuration: both mqtt_client_cert_path and \
                         mqtt_client_key_path must be provided for client authentication. \
                         Continuing without client auth."
                    );
                    None
                }
            };

            Some(TlsConfig {
                ca_cert,
                client_auth,
            })
        } else {
            None
        };

        match Notifier::new(
            mqtt_url,
            cli.mqtt_username,
            cli.mqtt_password,
            cli.mqtt_topic,
            tls_config,
        ) {
            Ok((n, mut eventloop)) => {
                notifier = Some(n);
                let mqtt_cancel_token = cancel_token.clone();
                tokio::spawn(async move {
                    use rumqttc::{Event, Packet};
                    let mut consecutive_errors: u32 = 0;
                    loop {
                        tokio::select! {
                            () = mqtt_cancel_token.cancelled() => {
                                info!("MQTT event loop shutting down");
                                break;
                            }
                            result = eventloop.poll() => {
                                match result {
                                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                                        if consecutive_errors > 0 {
                                            info!(
                                                previous_errors = consecutive_errors,
                                                "MQTT connection restored"
                                            );
                                        }
                                        consecutive_errors = 0;
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        consecutive_errors = consecutive_errors.saturating_add(1);

                                        if consecutive_errors == 1 {
                                            error!(error = ?e, "MQTT connection error");
                                        } else {
                                            debug!(
                                                error = ?e,
                                                consecutive_errors,
                                                "MQTT still disconnected"
                                            );
                                        }

                                        let backoff_ms = MQTT_INITIAL_BACKOFF_MS
                                            .saturating_mul(2_u64.saturating_pow(consecutive_errors.saturating_sub(1)))
                                            .min(MQTT_MAX_BACKOFF_MS);

                                        // Use select to allow cancellation during backoff sleep
                                        tokio::select! {
                                            () = mqtt_cancel_token.cancelled() => {
                                                info!("MQTT event loop shutting down during backoff");
                                                break;
                                            }
                                            () = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => {
                error!("Failed to initialize notifier: {:?}", e);
                return Err(e);
            }
        }
    }

    tokio::try_join!(
        start_main_server(
            &cli.listen_addr,
            Arc::clone(&fm),
            notifier,
            main_server_cancel_token
        ),
        start_metrics_server(&cli.metrics_listen_addr, metrics_server_cancel_token),
    )?;

    // Waits for signal before exiting gracefully
    ctrl_c_listener_task.await?;

    info!("All services shut down gracefully.");

    Ok(())
}

async fn start_main_server(
    listen_address: &str,
    firmware_manager: Arc<FirmwareManager>,
    notifier: Option<Notifier>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let listener = TcpListener::bind(listen_address).await?;
    info!("OtaFlux listening on {}", listener.local_addr()?);

    let shutdown_future = async move {
        cancel_token.cancelled().await;
    };

    axum::serve(listener, api_router(firmware_manager, notifier))
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
