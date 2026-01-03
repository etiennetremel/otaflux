use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::instrument;

use crate::firmware_manager::FirmwareManager;

#[derive(Deserialize)]
pub struct DeviceParams {
    device: Option<String>,
}

/// Returns the firmware version, CRC32, and size for the specified device.
#[instrument(skip(manager, params))]
pub async fn version_handler(
    State(manager): State<Arc<FirmwareManager>>,
    Query(params): Query<DeviceParams>,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );

    let Some(device) = params.device.filter(|d| !d.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            headers,
            "Missing required query parameter: 'device'".to_string(),
        );
    };

    if let Ok(fw) = manager.get_firmware(&device).await {
        let body = format!("{}\n{}\n{}", fw.version, fw.crc, fw.size);
        (StatusCode::OK, headers, body)
    } else {
        let body = format!("No firmware for device '{device}'");
        (StatusCode::NOT_FOUND, headers, body)
    }
}

/// Returns the firmware binary for the specified device.
#[instrument(skip(manager, params))]
pub async fn firmware_handler(
    State(manager): State<Arc<FirmwareManager>>,
    Query(params): Query<DeviceParams>,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();

    let Some(device) = params.device.filter(|d| !d.is_empty()) else {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        return (
            StatusCode::BAD_REQUEST,
            headers,
            "Missing required query parameter: 'device'".into(),
        );
    };

    if let Ok(fw) = manager.get_firmware(&device).await {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        (StatusCode::OK, headers, fw.binary.clone())
    } else {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        (
            StatusCode::NOT_FOUND,
            headers,
            format!("No firmware for device '{device}'").into(),
        )
    }
}

/// Returns 200 OK if the server is healthy.
pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
