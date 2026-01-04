# MQTT Notifications

OtaFlux publishes firmware update notifications to an MQTT broker when new
images are pushed via webhooks. Devices subscribe to topics and receive
immediate notification of available updates, eliminating the need for polling.

## Overview

```mermaid
sequenceDiagram
    participant Registry as OCI Registry
    participant OtaFlux as OtaFlux
    participant MQTT as MQTT Broker
    participant Device as IoT Device

    Registry->>OtaFlux: Webhook (new image pushed)
    OtaFlux->>Registry: Fetch firmware metadata
    OtaFlux->>MQTT: Publish notification
    MQTT->>Device: Deliver message
    Device->>OtaFlux: GET /firmware?device=...
    OtaFlux-->>Device: Firmware binary
```

## Configuration

### Basic Configuration

Enable MQTT with the `--mqtt-url` flag:

```bash
otaflux \
    --registry-url "https://your-registry.example.com" \
    --repository-prefix "my-project/" \
    --registry-username "username" \
    --registry-password "password" \
    --mqtt-url "mqtt://mqtt-broker:1883?client_id=otaflux" \
    --mqtt-topic "firmware/updates"
```

### MQTT URL Format

The MQTT URL follows this format:

```
mqtt[s]://[username:password@]host:port[?client_id=<id>]
```

**Examples:**

| URL | Description |
|-----|-------------|
| `mqtt://broker:1883` | Plain MQTT |
| `mqtts://broker:8883` | MQTT over TLS |
| `mqtt://broker:1883?client_id=otaflux` | With client ID |
| `mqtt://user:pass@broker:1883` | With credentials in URL |

> **Note**: Prefer using `--mqtt-username` and `--mqtt-password` flags instead
> of embedding credentials in the URL.

### TLS Configuration

For secure connections with server certificate validation:

```bash
otaflux \
    --mqtt-url "mqtts://mqtt-broker:8883?client_id=otaflux" \
    --mqtt-ca-cert-path "/etc/otaflux/certs/ca.crt" \
    ...
```

For mutual TLS (mTLS) with client certificates:

```bash
otaflux \
    --mqtt-url "mqtts://mqtt-broker:8883?client_id=otaflux" \
    --mqtt-ca-cert-path "/etc/otaflux/certs/ca.crt" \
    --mqtt-client-cert-path "/etc/otaflux/certs/client.crt" \
    --mqtt-client-key-path "/etc/otaflux/certs/client.key" \
    ...
```

> **Important**: All three TLS options must be provided together for mTLS. If
> only some are set, OtaFlux logs a warning and continues without client
> authentication.

### Docker Example

```bash
docker run -ti --rm \
    -v $PWD/certs:/etc/otaflux/certs:ro \
    -p 8080:8080 \
    -p 9090:9090 \
    ghcr.io/etiennetremel/otaflux \
        --registry-url "https://your-registry.example.com" \
        --repository-prefix "my-project/" \
        --registry-username "username" \
        --registry-password "password" \
        --mqtt-url "mqtts://mqtt-broker:8883?client_id=otaflux" \
        --mqtt-username "otaflux" \
        --mqtt-password "secret" \
        --mqtt-topic "firmware/updates" \
        --mqtt-ca-cert-path "/etc/otaflux/certs/ca.crt" \
        --mqtt-client-cert-path "/etc/otaflux/certs/client.crt" \
        --mqtt-client-key-path "/etc/otaflux/certs/client.key"
```

## Message Format

OtaFlux publishes JSON messages when new firmware is available:

```json
{
  "version": "1.0.0",
  "size": 942320
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | Semantic version of the firmware |
| `size` | number | Size of the firmware binary in bytes |

## Topic Structure

Topics follow this pattern:

```
{mqtt-topic}/{device-id}
```

**Example:**

With `--mqtt-topic "firmware/updates"` and device `esp32-sensor`:

```
firmware/updates/esp32-sensor
```

### Wildcards

Devices can subscribe to multiple topics using MQTT wildcards:

| Pattern | Description |
|---------|-------------|
| `firmware/updates/+` | All devices at one level |
| `firmware/updates/#` | All devices at all levels |
| `firmware/updates/esp32-+` | All ESP32 devices |

## Quality of Service (QoS)

OtaFlux publishes messages with **QoS 1 (At Least Once)**, ensuring:

- Messages are delivered at least once
- Broker acknowledges receipt
- Messages may be duplicated (devices should handle idempotently)

### Message Retention

Messages are published with the **retain flag set to `true`**, meaning:

- New subscribers immediately receive the last known firmware version
- Devices connecting after a push still get the notification
- Useful for devices that were offline during the push

## Device Integration

### Rust Example

```rust
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde::Deserialize;

#[derive(Deserialize)]
struct FirmwareUpdate {
    version: String,
    size: u64,
}

async fn subscribe_to_updates(client: &AsyncClient, device_id: &str) {
    let topic = format!("firmware/updates/{}", device_id);
    client
        .subscribe(&topic, QoS::AtLeastOnce)
        .await
        .expect("Failed to subscribe");
}

async fn handle_message(payload: &[u8], current_version: &str) {
    let update: FirmwareUpdate = serde_json::from_slice(payload)
        .expect("Failed to parse firmware update");
    
    if update.version != current_version {
        println!("New firmware: {} ({} bytes)", update.version, update.size);
        // Trigger OTA update...
    }
}
```

## Troubleshooting

### Connection Issues

| Symptom | Possible Cause | Solution |
|---------|---------------|----------|
| `MQTT event loop error` in logs | Broker unreachable | Verify broker host/port, check firewall |
| Connection drops repeatedly | Keep-alive timeout | Increase broker keep-alive settings |
| TLS handshake failure | Certificate mismatch | Verify CA cert matches broker's cert |
| Authentication failed | Wrong credentials | Check `--mqtt-username` and `--mqtt-password` |

### Debug MQTT

Enable debug logging to see MQTT activity:

```bash
otaflux --log-level debug ...
```

Look for log entries like:

```
INFO  Publishing payload to topic "firmware/updates/esp32-sensor": ...
```

### Testing with mosquitto_sub

Verify messages are being published:

```bash
# Subscribe to all firmware updates
mosquitto_sub -h mqtt-broker -t "firmware/updates/#" -v

# With authentication
mosquitto_sub -h mqtt-broker -u user -P password -t "firmware/updates/#" -v

# With TLS
mosquitto_sub -h mqtt-broker -p 8883 --cafile ca.crt -t "firmware/updates/#" -v
```

## Best Practices

1. **Use TLS in production** - Always use `mqtts://` with proper certificates
2. **Set a unique client ID** - Prevents connection conflicts
3. **Handle duplicates** - QoS 1 may deliver messages multiple times
4. **Implement reconnection** - Network failures happen; auto-reconnect
5. **Use retained messages** - Devices offline during push still get updates
6. **Monitor broker health** - Set up alerts for broker availability
