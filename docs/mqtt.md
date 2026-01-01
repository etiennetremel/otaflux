# MQTT Notifications

OtaFlux can publish firmware update notifications to an MQTT broker when new
firmware is pushed to the registry. This enables push-based updates where
devices subscribe to topics and receive immediate notification of available
updates.

## Configuration

Enable MQTT by providing the `--mqtt-url` flag:

```bash
podman run -ti --rm \
    -p 8080:8080 \
    -p 9090:9090 \
    ghcr.io/etiennetremel/otaflux \
        --registry-url "https://your-registry.example.com" \
        --repository-prefix "my-project/" \
        --registry-username "username" \
        --registry-password "password" \
        --mqtt-url "mqtt://mqtt-broker:1883?client_id=otaflux" \
        --mqtt-topic "firmware/updates"
```

## MQTT with TLS

For secure MQTT connections with client certificates, provide all three TLS
options:

```bash
podman run -ti --rm \
    -v $PWD/certs:/etc/otaflux/certs:ro \
    -p 8080:8080 \
    -p 9090:9090 \
    ghcr.io/etiennetremel/otaflux \
        --registry-url "https://your-registry.example.com" \
        --repository-prefix "my-project/" \
        --registry-username "username" \
        --registry-password "password" \
        --mqtt-url "mqtts://mqtt-broker:8883?client_id=otaflux" \
        --mqtt-topic "firmware/updates" \
        --mqtt-ca-cert-path "/etc/otaflux/certs/ca.crt" \
        --mqtt-client-cert-path "/etc/otaflux/certs/client.crt" \
        --mqtt-client-key-path "/etc/otaflux/certs/client.key"
```

> **Note**: All three TLS options (`--mqtt-ca-cert-path`, `--mqtt-client-cert-path`,
> `--mqtt-client-key-path`) must be provided together. If only some are provided,
> OtaFlux will log a warning and continue without TLS.

## Message Format

When a firmware update is available, OtaFlux publishes a JSON message to the
topic `{mqtt-topic}/{device-id}`:

```json
{
  "version": "1.0.0",
  "size": 942320
}
```

## Topic Structure

Messages are published to topics following this pattern:

```
{mqtt-topic}/{repository-path}/{device-name}
```

For example, with `--mqtt-topic "firmware/updates"` and a device at
`my-project/esp32-sensor`, the topic would be:

```
firmware/updates/my-project/esp32-sensor
```

## Device Integration

Devices should subscribe to their specific topic to receive update notifications:

```rust
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde::Deserialize;

#[derive(Deserialize)]
struct FirmwareUpdate {
    version: String,
    size: u64,
}

async fn subscribe_to_updates(client: &AsyncClient) {
    client
        .subscribe("firmware/updates/my-project/esp32-sensor", QoS::AtLeastOnce)
        .await
        .unwrap();
}

async fn handle_message(payload: &[u8], current_version: &str) {
    let update: FirmwareUpdate = serde_json::from_slice(payload).unwrap();
    
    // Compare with current version and trigger OTA if needed
    if update.version != current_version {
        println!("New firmware available: {} ({} bytes)", update.version, update.size);
        start_ota_update(&update.version).await;
    }
}
```
