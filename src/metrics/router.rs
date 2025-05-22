use axum::{routing::get, Router};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::future::ready;

// Creates a new Axum Router for the /metrics endpoint.
// This endpoint will expose metrics in the Prometheus format.
pub fn metrics_router() -> Router {
    let recorder_handle = setup_metrics_recorder();
    Router::new().route("/metrics", get(move || ready(recorder_handle.render())))
}

// Sets up the Prometheus recorder with specific buckets for HTTP request durations.
// This function initializes the metrics recorder and installs it globally.
fn setup_metrics_recorder() -> PrometheusHandle {
    const EXPONENTIAL_SECONDS: &[f64] = &[
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_requests_duration_seconds".to_string()),
            EXPONENTIAL_SECONDS,
        )
        .unwrap()
        .install_recorder()
        .unwrap()
}
