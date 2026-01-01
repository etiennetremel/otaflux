//! Firmware endpoint integration tests.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use otaflux::api::router::api_router;
use std::sync::Arc;
use tower::ServiceExt;

use common::{body_to_bytes, create_app, init_tracing, MockRegistryBuilder, TestFirmware};

#[tokio::test]
async fn test_firmware_endpoint_returns_binary() {
    init_tracing();

    let firmware_content = b"actual firmware binary data here";
    let firmware = TestFirmware::new("device-002", "2.0.0", firmware_content);
    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(firmware)
        .await
        .build()
        .await;

    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/firmware?device=device-002")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("")),
        Some("application/octet-stream"),
        "Content-Type should be octet-stream"
    );

    let body = body_to_bytes(response.into_body()).await;
    assert_eq!(body, firmware_content, "Firmware binary should match");
}

#[tokio::test]
async fn test_firmware_endpoint_device_not_found() {
    init_tracing();

    let registry = MockRegistryBuilder::new().await.build().await;
    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/firmware?device=nonexistent-device")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_firmware_cache_hit() {
    init_tracing();

    let firmware = TestFirmware::new("device-cache", "1.0.0", b"cached firmware");
    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(firmware.clone())
        .await
        .build()
        .await;

    // Use the same firmware manager for both requests to test caching
    let fm = registry.firmware_manager();

    // First request - should fetch from registry
    let app1 = api_router(Arc::clone(&fm), None);
    let request1 = Request::builder()
        .uri("/firmware?device=device-cache")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response1 = app1.oneshot(request1).await.expect("send request");
    assert_eq!(response1.status(), StatusCode::OK);
    let body1 = body_to_bytes(response1.into_body()).await;
    assert_eq!(body1, firmware.bytes);

    // Second request - should hit cache (same firmware manager)
    let app2 = api_router(Arc::clone(&fm), None);
    let request2 = Request::builder()
        .uri("/firmware?device=device-cache")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response2 = app2.oneshot(request2).await.expect("send request");
    assert_eq!(response2.status(), StatusCode::OK);
    let body2 = body_to_bytes(response2.into_body()).await;
    assert_eq!(body2, firmware.bytes);

    // Both should return the same data
    assert_eq!(body1, body2, "Cached response should match original");
}
