use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use otaflux::api::router::api_router;
use otaflux::firmware_manager::FirmwareManager;
use otaflux::notifier::Notifier;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use tower::ServiceExt;
use tracing::debug;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

// Custom responder to log any unmatched request
struct LoggingResponder;

impl Respond for LoggingResponder {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        println!(
            "UNMATCHED REQUEST: {} {}",
            request.method,
            request.url.path()
        );
        ResponseTemplate::new(404)
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_harbor_webhook_triggers_mqtt_notification() {
    // Enable tracing for debug output
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .try_init();

    // 1. Start mock OCI registry
    let mock_registry = MockServer::start().await;
    let registry_uri = mock_registry.uri();
    // Extract host:port from the URI (e.g., "127.0.0.1:12345")
    let registry_host_port = registry_uri
        .strip_prefix("http://")
        .unwrap_or(&registry_uri);

    // Mock GET /v2/repo/device-123/tags/list
    Mock::given(method("GET"))
        .and(path("/v2/repo/device-123/tags/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "repo/device-123",
            "tags": ["1.0.0"]
        })))
        .mount(&mock_registry)
        .await;

    // Mock GET /v2/repo/device-123/manifests/1.0.0 (returns OCI image manifest)
    // We need to declare firmware_bytes early to get the correct size for the manifest
    let firmware_bytes = b"fake firmware binary data for testing";
    let firmware_len = firmware_bytes.len();

    // Compute the actual SHA256 digest of the firmware bytes - oci-client validates this!
    let mut firmware_hasher = Sha256::new();
    firmware_hasher.update(firmware_bytes);
    let firmware_digest = format!("sha256:{:x}", firmware_hasher.finalize());
    debug!("Firmware digest: {}", firmware_digest);

    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": "sha256:configdigest",
            "size": 100
        },
        "layers": [
            {
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": firmware_digest.clone(),
                "size": firmware_len
            }
        ]
    });

    // Serialize the manifest to compute its SHA256 digest
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let mut manifest_hasher = Sha256::new();
    manifest_hasher.update(&manifest_bytes);
    let manifest_digest = format!("sha256:{:x}", manifest_hasher.finalize());

    debug!("Manifest digest: {}", manifest_digest);
    debug!(
        "Manifest JSON: {}",
        String::from_utf8_lossy(&manifest_bytes)
    );

    Mock::given(method("GET"))
        .and(path("/v2/repo/device-123/manifests/1.0.0"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.oci.image.manifest.v1+json")
                .insert_header("Docker-Content-Digest", manifest_digest.clone())
                .set_body_bytes(manifest_bytes),
        )
        .mount(&mock_registry)
        .await;

    // Mock GET /v2/repo/device-123/blobs/<firmware_digest> (firmware binary)
    Mock::given(method("GET"))
        .and(path(format!("/v2/repo/device-123/blobs/{firmware_digest}")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/octet-stream")
                .insert_header("Content-Length", firmware_len.to_string())
                .insert_header("Docker-Content-Digest", firmware_digest.clone())
                .set_body_bytes(firmware_bytes.to_vec()),
        )
        .expect(1) // Expect this to be called exactly once
        .mount(&mock_registry)
        .await;

    // Mock /v2/ base endpoint (required for OCI registry auth handshake)
    Mock::given(method("GET"))
        .and(path("/v2/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_registry)
        .await;

    // Catch-all for any unmatched requests (for debugging)
    Mock::given(wiremock::matchers::any())
        .respond_with(LoggingResponder)
        .mount(&mock_registry)
        .await;

    // 2. Start MQTT broker using testcontainers
    let mosquitto_node = Mosquitto::default().start().await.unwrap();
    let mqtt_port = mosquitto_node.get_host_port_ipv4(1883).await.unwrap();
    let mqtt_url = format!("mqtt://127.0.0.1:{mqtt_port}?client_id=otaflux-publisher");

    // 3. Setup a subscriber to verify the message
    let mut mqttoptions = MqttOptions::new("test-subscriber", "127.0.0.1", mqtt_port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

    // Subscribe to the topic that matches the device
    // Based on harbor.rs, topic is `{prefix}/{device_id}` where device_id is repo_full_name
    client
        .subscribe("otaflux/repo/device-123", QoS::AtMostOnce)
        .await
        .unwrap();

    // Spawn a task to listen for the message
    let (tx, mut rx) = tokio::sync::mpsc::channel::<rumqttc::Publish>(1);
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(notification) => {
                    if let rumqttc::Event::Incoming(rumqttc::Packet::Publish(p)) = notification {
                        let _ = tx.send(p).await;
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("MQTT eventloop error: {e:?}");
                }
            }
        }
    });

    // Give the subscriber time to connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Create FirmwareManager pointing to mock registry
    // Note: oci-client expects registry without protocol - it uses the `insecure` flag to determine http vs https
    let fm = Arc::new(
        FirmwareManager::new(
            registry_host_port.to_string(), // just host:port, no http://
            "user".to_string(),
            "pass".to_string(),
            true, // insecure (HTTP)
            "",   // no prefix - repo_full_name includes the full path
            None, // no cosign verification
        )
        .unwrap(),
    );

    // 5. Create Notifier (no TLS for test container)
    let (notifier, mut notifier_eventloop) = Notifier::new(
        mqtt_url.clone(),
        String::new(), // no auth for mosquitto test container
        String::new(),
        "otaflux".to_string(),
        None, // no TLS
    )
    .expect("Failed to create Notifier");

    // Spawn a task to drive the notifier's MQTT event loop
    tokio::spawn(async move {
        loop {
            if let Err(e) = notifier_eventloop.poll().await {
                eprintln!("Notifier eventloop error: {e:?}");
                break;
            }
        }
    });

    let notifier = Some(notifier);

    // 6. Create the app router
    let app = api_router(fm, notifier);

    // 7. Send Webhook Request matching Harbor's format
    let payload = serde_json::json!({
        "type": "PUSH_ARTIFACT",
        "occur_at": 123_456_789,
        "operator": "admin",
        "event_data": {
            "resources": [
                {
                    "digest": "sha256:abc123",
                    "tag": "1.0.0",
                    "resource_url": format!("{}/repo/device-123:1.0.0", registry_host_port)
                }
            ],
            "repository": {
                "date_created": 123_456_789,
                "name": "device-123",
                "namespace": "repo",
                "repo_full_name": "repo/device-123",
                "repo_type": "private"
            }
        }
    });

    let request = Request::builder()
        .uri("/webhooks/harbor")
        .method("POST")
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 8. Assert MQTT message received
    let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

    assert!(result.is_ok(), "Timed out waiting for MQTT message");
    let packet = result.unwrap().expect("Channel closed without message");

    assert_eq!(packet.topic, "otaflux/repo/device-123");

    // Verify the payload contains expected firmware info
    let payload: serde_json::Value = serde_json::from_slice(&packet.payload).unwrap();
    assert_eq!(payload["version"], "1.0.0");
    assert_eq!(payload["size"], firmware_bytes.len());
}
