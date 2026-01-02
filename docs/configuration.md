# Configuration

Configure OtaFlux with command-line flags or environment variables. Environment
variables take precedence when both are provided.

## Command-Line Options

### Required Options

| Flag | Environment Variable | Description |
|------|---------------------|-------------|
| `--registry-url` | `REGISTRY_URL` | OCI registry URL (e.g., `https://registry.example.com`) |
| `--repository-prefix` | `REPOSITORY_PREFIX` | Repository prefix for firmware images (e.g., `my-project/`) |
| `--registry-username` | `REGISTRY_USERNAME` | Registry authentication username |
| `--registry-password` | `REGISTRY_PASSWORD` | Registry authentication password |

### Optional Options

| Flag | Environment Variable | Description | Default |
|------|---------------------|-------------|---------|
| `--registry-insecure` | `REGISTRY_INSECURE` | Use HTTP instead of HTTPS | `false` |
| `--cosign-pub-key-path` | `COSIGN_PUB_KEY_PATH` | Path to Cosign public key for signature verification | - |
| `--listen-addr` | `LISTEN_ADDR` | HTTP server bind address | `0.0.0.0:8080` |
| `--metrics-listen-addr` | `METRICS_LISTEN_ADDR` | Metrics server bind address | `0.0.0.0:9090` |
| `--log-level` | `LOG_LEVEL` | Log verbosity (trace, debug, info, warn, error) | `info` |

### MQTT Options

| Flag | Environment Variable | Description | Default |
|------|---------------------|-------------|---------|
| `--mqtt-url` | `MQTT_URL` | MQTT broker URL (e.g., `mqtt://broker:1883?client_id=otaflux`) | - |
| `--mqtt-username` | `MQTT_USERNAME` | MQTT authentication username | `""` |
| `--mqtt-password` | `MQTT_PASSWORD` | MQTT authentication password | `""` |
| `--mqtt-topic` | `MQTT_TOPIC` | Base topic prefix for notifications | `""` |
| `--mqtt-ca-cert-path` | `MQTT_CA_CERT_PATH` | Path to CA certificate for TLS | - |
| `--mqtt-client-cert-path` | `MQTT_CLIENT_CERT_PATH` | Path to client certificate for mTLS | - |
| `--mqtt-client-key-path` | `MQTT_CLIENT_KEY_PATH` | Path to client private key for mTLS | - |

> **Note**: For mTLS, all three certificate options (`--mqtt-ca-cert-path`,
> `--mqtt-client-cert-path`, `--mqtt-client-key-path`) must be provided together.

## Configuration Examples

### Minimal Configuration

```bash
otaflux \
    --registry-url "https://ghcr.io" \
    --repository-prefix "myorg/firmware/" \
    --registry-username "user" \
    --registry-password "token"
```

### Full Configuration with MQTT and Cosign

```bash
otaflux \
    --registry-url "https://registry.example.com" \
    --repository-prefix "iot-devices/" \
    --registry-username "admin" \
    --registry-password "secret" \
    --cosign-pub-key-path "/etc/otaflux/cosign.pub" \
    --mqtt-url "mqtts://mqtt.example.com:8883?client_id=otaflux" \
    --mqtt-username "otaflux" \
    --mqtt-password "mqtt-secret" \
    --mqtt-topic "firmware/updates" \
    --mqtt-ca-cert-path "/etc/otaflux/certs/ca.crt" \
    --mqtt-client-cert-path "/etc/otaflux/certs/client.crt" \
    --mqtt-client-key-path "/etc/otaflux/certs/client.key" \
    --log-level "info"
```

### Using Environment Variables

```bash
export REGISTRY_URL="https://ghcr.io"
export REPOSITORY_PREFIX="myorg/firmware/"
export REGISTRY_USERNAME="user"
export REGISTRY_PASSWORD="token"
export LOG_LEVEL="debug"

otaflux
```

## HTTP API Reference

OtaFlux exposes two HTTP servers:
- **Main API** (default: port 8080) - Device endpoints and webhooks
- **Metrics** (default: port 9090) - Prometheus metrics

### Device Endpoints

#### Health Check

```http
GET /health
```

Returns server health status.

| Response Code | Description |
|---------------|-------------|
| `200 OK` | Server is healthy |

**Example:**

```bash
curl http://localhost:8080/health
```

---

#### Get Firmware Version

```http
GET /version?device=<device-id>
```

Returns the latest firmware version, CRC32 checksum, and size for the specified device.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `device` | string | Yes | Device identifier (repository name) |

**Response Format:**

```
<version>
<crc32>
<size>
```

| Response Code | Description |
|---------------|-------------|
| `200 OK` | Firmware found |
| `404 Not Found` | No firmware available for device |

**Example:**

```bash
curl 'http://localhost:8080/version?device=esp32-sensor'

# Response:
1.2.3
4051932293
942320
```

---

#### Download Firmware

```http
GET /firmware?device=<device-id>
```

Downloads the firmware binary for the specified device.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `device` | string | Yes | Device identifier (repository name) |

**Response Headers:**

| Header | Value |
|--------|-------|
| `Content-Type` | `application/octet-stream` |

| Response Code | Description |
|---------------|-------------|
| `200 OK` | Firmware binary returned |
| `404 Not Found` | No firmware available for device |

**Example:**

```bash
curl -o firmware.bin 'http://localhost:8080/firmware?device=esp32-sensor'
```

---

### Webhook Endpoints

#### Harbor Webhook

```http
POST /webhooks/harbor
```

Receives webhook events from Harbor registry. When a `PUSH_ARTIFACT` event is
received, OtaFlux fetches the new firmware and publishes an MQTT notification
(if configured).

**Request Body:** Harbor webhook payload (JSON)

| Response Code | Description |
|---------------|-------------|
| `200 OK` | Webhook processed |

See [Harbor Webhooks](webhooks.md) for detailed setup instructions.

---

### Metrics Endpoint

```http
GET /metrics
```

Returns Prometheus-formatted metrics. Served on the metrics port (default: 9090).

**Available Metrics:**

| Metric | Type | Description |
|--------|------|-------------|
| `firmware_cache_hit_total` | Counter | Cache hits by device |
| `firmware_cache_miss_total` | Counter | Cache misses by device |
| `http_requests_total` | Counter | Total HTTP requests |
| `http_request_duration_seconds` | Histogram | Request latency |

**Example:**

```bash
curl http://localhost:9090/metrics
```

## Device Integration

### ESP32 Example (ESP-IDF)

```c
#include "esp_http_client.h"
#include "esp_ota_ops.h"

#define OTAFLUX_URL "http://otaflux.local:8080"
#define DEVICE_ID "esp32-sensor"

void check_for_update(void) {
    char url[128];
    snprintf(url, sizeof(url), "%s/version?device=%s", OTAFLUX_URL, DEVICE_ID);
    
    esp_http_client_config_t config = {
        .url = url,
    };
    
    esp_http_client_handle_t client = esp_http_client_init(&config);
    esp_http_client_perform(client);
    
    // Parse response: version\ncrc32\nsize
    // Compare with current version and trigger OTA if needed
    
    esp_http_client_cleanup(client);
}
```

### Arduino Example

```cpp
#include <HTTPClient.h>
#include <Update.h>

const char* OTAFLUX_URL = "http://otaflux.local:8080";
const char* DEVICE_ID = "arduino-device";
const char* CURRENT_VERSION = "1.0.0";

void checkForUpdate() {
    HTTPClient http;
    String url = String(OTAFLUX_URL) + "/version?device=" + DEVICE_ID;
    
    http.begin(url);
    int httpCode = http.GET();
    
    if (httpCode == 200) {
        String response = http.getString();
        // Parse version from first line
        String newVersion = response.substring(0, response.indexOf('\n'));
        
        if (newVersion != CURRENT_VERSION) {
            // Download and apply firmware
            performOTA();
        }
    }
    
    http.end();
}
```

## Repository Naming Convention

OtaFlux constructs the full repository path as:

```
{registry-url}/{repository-prefix}{device-id}
```

**Example:**

- Registry URL: `https://ghcr.io`
- Repository prefix: `myorg/firmware/`
- Device ID: `esp32-sensor`
- Full path: `ghcr.io/myorg/firmware/esp32-sensor`

## Semantic Versioning

OtaFlux uses semantic versioning (semver) to determine the latest firmware version.
Tags must follow the semver format:

- `1.0.0`
- `v1.0.0`
- `1.2.3-beta.1`
- `2.0.0-rc.1+build.123`

Non-semver tags (e.g., `latest`, `dev`, `main`) are ignored.
