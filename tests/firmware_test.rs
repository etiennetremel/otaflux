//! Firmware endpoint integration tests.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use otaflux::api::router::api_router;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
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

/// Concurrent requests for the same device should trigger only one registry fetch.
#[tokio::test]
async fn test_thundering_herd_protection() {
    init_tracing();

    let blob_fetch_count = Arc::new(AtomicUsize::new(0));
    let firmware = TestFirmware::new("device-herd", "1.0.0", b"thundering herd firmware");

    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware_delayed(
            firmware.clone(),
            Duration::from_millis(50),
            Arc::clone(&blob_fetch_count),
        )
        .await
        .build()
        .await;

    let fm = registry.firmware_manager();
    let num_concurrent_requests = 10;
    let mut join_set = JoinSet::new();

    for _ in 0..num_concurrent_requests {
        let fm_clone = Arc::clone(&fm);
        let device_id = firmware.device_id.clone();
        join_set.spawn(async move { fm_clone.get_firmware(&device_id).await });
    }

    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        results.push(result);
    }

    for (i, result) in results.iter().enumerate() {
        let fw = result
            .as_ref()
            .expect("task should not panic")
            .as_ref()
            .expect("request should succeed");

        assert_eq!(
            fw.binary.as_ref(),
            firmware.bytes.as_slice(),
            "Request {i} should get correct firmware"
        );
    }

    let actual_fetches = blob_fetch_count.load(Ordering::SeqCst);
    assert_eq!(
        actual_fetches, 1,
        "Blob endpoint should be called exactly once, but was called {actual_fetches} times"
    );
}
