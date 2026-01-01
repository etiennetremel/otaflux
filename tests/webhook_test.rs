//! Harbor webhook integration tests.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rumqttc::{AsyncClient, MqttOptions, QoS};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use tower::ServiceExt;

use common::{create_app, create_app_with_mqtt, init_tracing, MockRegistryBuilder, TestFirmware};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_harbor_webhook_triggers_mqtt_notification() {
    init_tracing();

    // Setup mock registry with firmware
    // Note: device_id must match what webhook handler extracts from repository.name
    let firmware = TestFirmware::new("device-123", "1.0.0", b"webhook test firmware");
    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(firmware.clone())
        .await
        .build()
        .await;

    // Start MQTT broker
    let mosquitto = Mosquitto::default().start().await.expect("start mosquitto");
    let mqtt_port = mosquitto.get_host_port_ipv4(1883).await.expect("get port");

    // Setup MQTT subscriber to verify the notification
    let mut mqttoptions = MqttOptions::new("test-subscriber", "127.0.0.1", mqtt_port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

    client
        .subscribe("otaflux/device-123", QoS::AtMostOnce)
        .await
        .expect("subscribe");

    // Channel to receive published messages
    let (tx, mut rx) = tokio::sync::mpsc::channel::<rumqttc::Publish>(1);
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(p))) => {
                    let _ = tx.send(p).await;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("MQTT error: {e:?}");
                    break;
                }
            }
        }
    });

    // Wait for subscriber to connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create app with MQTT notifier
    let (app, _handle) = create_app_with_mqtt(registry.firmware_manager(), mqtt_port);

    // Send Harbor webhook
    let webhook_payload = serde_json::json!({
        "type": "PUSH_ARTIFACT",
        "occur_at": 1_234_567_890,
        "operator": "admin",
        "event_data": {
            "resources": [{
                "digest": "sha256:abc123",
                "tag": "1.0.0",
                "resource_url": format!("{}/repo/device-123:1.0.0", registry.host_port())
            }],
            "repository": {
                "date_created": 1_234_567_890,
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
        .body(Body::from(
            serde_json::to_vec(&webhook_payload).expect("serialize"),
        ))
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");
    assert_eq!(response.status(), StatusCode::OK);

    // Verify MQTT message was published
    let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
    assert!(result.is_ok(), "Should receive MQTT message");

    let packet = result.unwrap().expect("message received");
    assert_eq!(packet.topic, "otaflux/device-123");

    let mqtt_payload: serde_json::Value =
        serde_json::from_slice(&packet.payload).expect("parse payload");
    assert_eq!(mqtt_payload["version"], "1.0.0");
    assert_eq!(mqtt_payload["size"], firmware.bytes.len());
}

#[tokio::test]
async fn test_harbor_webhook_ignores_non_push_events() {
    init_tracing();

    let registry = MockRegistryBuilder::new().await.build().await;
    let app = create_app(registry.firmware_manager());

    // Send DELETE_ARTIFACT event (should be ignored)
    let webhook_payload = serde_json::json!({
        "type": "DELETE_ARTIFACT",
        "occur_at": 1_234_567_890,
        "operator": "admin",
        "event_data": {
            "resources": [{
                "digest": "sha256:deleted",
                "tag": "1.0.0",
                "resource_url": "registry/repo/device:1.0.0"
            }],
            "repository": {
                "date_created": 1_234_567_890,
                "name": "device",
                "namespace": "repo",
                "repo_full_name": "repo/device",
                "repo_type": "private"
            }
        }
    });

    let request = Request::builder()
        .uri("/webhooks/harbor")
        .method("POST")
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&webhook_payload).expect("serialize"),
        ))
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    // Should still return OK (webhook received) but not process it
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_harbor_webhook_without_notifier() {
    init_tracing();

    // Note: device_id must match what webhook handler extracts from repository.name
    let firmware = TestFirmware::new("device-no-mqtt", "1.0.0", b"no mqtt firmware");
    let registry = MockRegistryBuilder::new()
        .await
        .with_firmware(firmware)
        .await
        .build()
        .await;

    // Create app WITHOUT notifier
    let app = create_app(registry.firmware_manager());

    let webhook_payload = serde_json::json!({
        "type": "PUSH_ARTIFACT",
        "occur_at": 1_234_567_890,
        "operator": "admin",
        "event_data": {
            "resources": [{
                "digest": "sha256:abc123",
                "tag": "1.0.0",
                "resource_url": "registry/repo/device-no-mqtt:1.0.0"
            }],
            "repository": {
                "date_created": 1_234_567_890,
                "name": "device-no-mqtt",
                "namespace": "repo",
                "repo_full_name": "repo/device-no-mqtt",
                "repo_type": "private"
            }
        }
    });

    let request = Request::builder()
        .uri("/webhooks/harbor")
        .method("POST")
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&webhook_payload).expect("serialize"),
        ))
        .expect("build request");

    let response = app.oneshot(request).await.expect("send request");

    // Should return OK even without notifier (graceful degradation)
    assert_eq!(response.status(), StatusCode::OK);
}
