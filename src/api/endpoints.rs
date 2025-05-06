use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use bytes::Bytes;
use serde::Deserialize;
use std::sync::Arc;

use crate::firmware::manager::FirmwareManager;

#[derive(Deserialize)]
pub struct DeviceParams {
    device: String,
}

pub async fn version_handler(
    State(manager): State<Arc<FirmwareManager>>,
    Query(DeviceParams { device }): Query<DeviceParams>,
) -> impl IntoResponse {
    if let Some(fw) = manager.get_current_firmware_for_device(&device).await {
        let body = format!("{}\n{}\n{}", fw.version, fw.crc, fw.size);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        (StatusCode::OK, headers, body)
    } else {
        (
            StatusCode::NOT_FOUND,
            HeaderMap::new(),
            format!("No firmware for device '{}'", device),
        )
    }
}

pub async fn firmware_handler(
    State(manager): State<Arc<FirmwareManager>>,
    Query(DeviceParams { device }): Query<DeviceParams>,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();

    if let Some(fw) = manager.get_current_firmware_for_device(&device).await {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        let body = Bytes::from(fw.binary.clone());
        (StatusCode::OK, headers, body)
    } else {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        let body = Bytes::from(format!("No firmware for device '{}'", device));
        (StatusCode::NOT_FOUND, headers, body)
    }
}

pub async fn health_handler() -> impl IntoResponse {
    StatusCode::OK
}
