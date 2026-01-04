//! Shared test utilities for `OtaFlux` integration tests.
//!
//! This module provides helpers for setting up mock OCI registries,
//! creating test applications, and common test utilities.

// Allow dead code since not all test files use all helpers
#![allow(dead_code)]

use axum::body::Body;
use http_body_util::BodyExt;
use otaflux::api::router::api_router;
use otaflux::firmware_manager::FirmwareManager;
use otaflux::notifier::Notifier;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Represents a firmware artifact for testing.
#[derive(Clone)]
pub struct TestFirmware {
    pub device_id: String,
    pub tag: String,
    pub bytes: Vec<u8>,
    pub digest: String,
}

impl TestFirmware {
    pub fn new(device_id: &str, tag: &str, bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = format!("sha256:{:x}", hasher.finalize());

        Self {
            device_id: device_id.to_string(),
            tag: tag.to_string(),
            bytes: bytes.to_vec(),
            digest,
        }
    }
}

/// Builder for setting up a mock OCI registry with firmware artifacts.
pub struct MockRegistryBuilder {
    server: MockServer,
    devices: HashMap<String, Vec<TestFirmware>>,
}

impl MockRegistryBuilder {
    pub async fn new() -> Self {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v2/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Self {
            server,
            devices: HashMap::new(),
        }
    }

    /// Adds a firmware artifact for a device with the given tag.
    pub async fn with_firmware(mut self, firmware: TestFirmware) -> Self {
        let device_id = firmware.device_id.clone();

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": "sha256:configdigest",
                "size": 100
            },
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": firmware.digest.clone(),
                "size": firmware.bytes.len()
            }]
        });

        let manifest_bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
        let mut manifest_hasher = Sha256::new();
        manifest_hasher.update(&manifest_bytes);
        let manifest_digest = format!("sha256:{:x}", manifest_hasher.finalize());

        Mock::given(method("GET"))
            .and(path(format!(
                "/v2/{}/manifests/{}",
                device_id, firmware.tag
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
                    .insert_header("Docker-Content-Digest", manifest_digest)
                    .set_body_bytes(manifest_bytes),
            )
            .mount(&self.server)
            .await;

        Mock::given(method("GET"))
            .and(path(format!("/v2/{}/blobs/{}", device_id, firmware.digest)))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/octet-stream")
                    .insert_header("Content-Length", firmware.bytes.len().to_string())
                    .insert_header("Docker-Content-Digest", firmware.digest.clone())
                    .set_body_bytes(firmware.bytes.clone()),
            )
            .mount(&self.server)
            .await;

        self.devices.entry(device_id).or_default().push(firmware);

        self
    }

    /// Adds a firmware artifact with a delayed blob response and fetch counter.
    ///
    /// Useful for testing concurrent request handling (e.g., thundering herd protection).
    pub async fn with_firmware_delayed(
        mut self,
        firmware: TestFirmware,
        delay: Duration,
        fetch_counter: Arc<AtomicUsize>,
    ) -> Self {
        let device_id = firmware.device_id.clone();

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": "sha256:configdigest",
                "size": 100
            },
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": firmware.digest.clone(),
                "size": firmware.bytes.len()
            }]
        });

        let manifest_bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
        let mut manifest_hasher = Sha256::new();
        manifest_hasher.update(&manifest_bytes);
        let manifest_digest = format!("sha256:{:x}", manifest_hasher.finalize());

        Mock::given(method("GET"))
            .and(path(format!(
                "/v2/{}/manifests/{}",
                device_id, firmware.tag
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
                    .insert_header("Docker-Content-Digest", manifest_digest)
                    .set_body_bytes(manifest_bytes),
            )
            .mount(&self.server)
            .await;

        let firmware_bytes = firmware.bytes.clone();
        Mock::given(method("GET"))
            .and(path_regex(format!(r"/v2/{device_id}/blobs/sha256:.*")))
            .respond_with(move |_req: &wiremock::Request| {
                fetch_counter.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/octet-stream")
                    .insert_header("Content-Length", firmware_bytes.len().to_string())
                    .set_body_bytes(firmware_bytes.clone())
                    .set_delay(delay)
            })
            .mount(&self.server)
            .await;

        self.devices.entry(device_id).or_default().push(firmware);

        self
    }

    /// Finalizes the mock registry setup and mounts the tags endpoint.
    pub async fn build(self) -> MockRegistry {
        for (device_id, firmwares) in &self.devices {
            let tags: Vec<&str> = firmwares.iter().map(|f| f.tag.as_str()).collect();

            Mock::given(method("GET"))
                .and(path(format!("/v2/{device_id}/tags/list")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "name": device_id,
                    "tags": tags
                })))
                .mount(&self.server)
                .await;
        }

        MockRegistry {
            server: self.server,
            devices: self.devices,
        }
    }
}

/// A configured mock OCI registry ready for testing.
pub struct MockRegistry {
    server: MockServer,
    devices: HashMap<String, Vec<TestFirmware>>,
}

impl MockRegistry {
    /// Returns the host:port string for the mock registry.
    pub fn host_port(&self) -> String {
        self.server
            .uri()
            .strip_prefix("http://")
            .unwrap_or(&self.server.uri())
            .to_string()
    }

    /// Creates a `FirmwareManager` configured to use this mock registry.
    pub fn firmware_manager(&self) -> Arc<FirmwareManager> {
        Arc::new(
            FirmwareManager::new(
                self.host_port(),
                "user".to_string(),
                "pass".to_string(),
                true,
                "",
                None,
            )
            .expect("create firmware manager"),
        )
    }
}

/// Creates a test app router without MQTT notifier.
pub fn create_app(fm: Arc<FirmwareManager>) -> axum::Router {
    api_router(fm, None)
}

/// Creates a test app router with MQTT notifier.
pub fn create_app_with_mqtt(
    fm: Arc<FirmwareManager>,
    mqtt_port: u16,
) -> (axum::Router, tokio::task::JoinHandle<()>) {
    let mqtt_url = format!("mqtt://127.0.0.1:{mqtt_port}?client_id=otaflux-publisher");

    let (notifier, mut eventloop) = Notifier::new(
        mqtt_url,
        String::new(),
        String::new(),
        "otaflux".to_string(),
        None,
    )
    .expect("create notifier");

    let handle = tokio::spawn(async move {
        loop {
            if let Err(e) = eventloop.poll().await {
                eprintln!("Notifier eventloop error: {e:?}");
                break;
            }
        }
    });

    (api_router(fm, Some(notifier)), handle)
}

/// Extracts response body as string.
pub async fn body_to_string(body: Body) -> String {
    let bytes = body.collect().await.expect("collect body").to_bytes();
    String::from_utf8(bytes.to_vec()).expect("body to string")
}

/// Extracts response body as bytes.
pub async fn body_to_bytes(body: Body) -> Vec<u8> {
    body.collect()
        .await
        .expect("collect body")
        .to_bytes()
        .to_vec()
}

/// Initialize tracing for tests (only once).
///
/// Defaults to `warn` level to reduce noise. Use `RUST_LOG=debug` for verbose output.
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string()))
        .with_test_writer()
        .try_init();
}
