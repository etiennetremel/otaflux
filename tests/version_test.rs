//! Version endpoint integration tests.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use common::{body_to_string, create_app, init_tracing, MockRegistryBuilder, TestFirmware};

#[tokio::test]
async fn test_version_endpoint_returns_firmware_info() {
    init_tracing();

    let firmware = TestFirmware::new("device-001", "1.2.3", b"firmware binary content");
    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(firmware.clone())
        .await
        .build()
        .await;

    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/version?device=device-001")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let lines: Vec<&str> = body.lines().collect();

    assert_eq!(lines.len(), 3, "Expected 3 lines: version, crc, size");
    assert_eq!(lines[0], "1.2.3", "Version should match");
    assert_eq!(
        lines[2],
        firmware.bytes.len().to_string(),
        "Size should match"
    );

    // Verify CRC is a valid number
    let crc: u32 = lines[1].parse().expect("CRC should be a number");
    assert_eq!(crc, crc32fast::hash(&firmware.bytes), "CRC should match");
}

#[tokio::test]
async fn test_version_endpoint_device_not_found() {
    init_tracing();

    let registry = MockRegistryBuilder::new().await.build().await;
    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/version?device=nonexistent-device")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = body_to_string(response.into_body()).await;
    assert!(
        body.contains("nonexistent-device"),
        "Error should mention device name"
    );
}

#[tokio::test]
async fn test_selects_highest_semver_tag() {
    init_tracing();

    // Add multiple versions - the highest (2.1.0) should be selected
    let fw_100 = TestFirmware::new("device-multi", "1.0.0", b"old firmware v1.0.0");
    let fw_150 = TestFirmware::new("device-multi", "1.5.0", b"firmware v1.5.0");
    let fw_210 = TestFirmware::new("device-multi", "2.1.0", b"latest firmware v2.1.0");
    let fw_200 = TestFirmware::new("device-multi", "2.0.0", b"firmware v2.0.0");

    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(fw_100)
        .await
        .with_firmware(fw_150)
        .await
        .with_firmware(fw_210)
        .await
        .with_firmware(fw_200)
        .await
        .build()
        .await;

    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/version?device=device-multi")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let version = body.lines().next().expect("first line is version");

    assert_eq!(version, "2.1.0", "Should select highest semver version");
}

#[tokio::test]
async fn test_ignores_non_semver_tags() {
    init_tracing();

    // Mix of semver and non-semver tags
    let fw_semver = TestFirmware::new("device-mixed", "1.0.0", b"valid semver firmware");
    let fw_latest = TestFirmware::new("device-mixed", "latest", b"latest tag firmware");

    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(fw_semver)
        .await
        .with_firmware(fw_latest)
        .await
        .build()
        .await;

    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/version?device=device-mixed")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let version = body.lines().next().expect("first line is version");

    assert_eq!(version, "1.0.0", "Should select only valid semver tag");
}

#[tokio::test]
async fn test_version_endpoint_missing_device_param() {
    init_tracing();

    let registry = MockRegistryBuilder::new().await.build().await;
    let app = create_app(registry.firmware_manager());

    let request = Request::builder()
        .uri("/version")
        .method("GET")
        .body(Body::empty())
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = body_to_string(response.into_body()).await;
    assert!(
        body.contains("device"),
        "Error should mention missing device parameter"
    );
}
