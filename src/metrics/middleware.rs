use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::IntoResponse,
};
use std::time::Instant;

// Middleware to track HTTP request metrics.
//
// This middleware records the total number of HTTP requests and the duration of each request.
// It uses the `metrics` crate to expose these metrics.
//
// The following metrics are recorded:
// - `http_requests_total`: A counter for the total number of HTTP requests, labeled by method, path, and status.
// - `http_requests_duration_seconds`: A histogram for the duration of HTTP requests, labeled by method, path, and status.
pub async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
    let start = Instant::now();
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        req.uri().path().to_owned()
    };
    let method = req.method().clone();

    let response = next.run(req).await;

    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    let labels = [
        ("method", method.to_string()),
        ("path", path),
        ("status", status),
    ];

    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_requests_duration_seconds", &labels).record(latency);

    response
}
