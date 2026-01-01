# Configuration

OtaFlux can be configured using command-line flags or environment variables.

## CLI Options

| Flag | Environment Variable | Description | Default |
|------|---------------------|-------------|---------|
| `--registry-url` | `REGISTRY_URL` | OCI registry URL | Required |
| `--repository-prefix` | `REPOSITORY_PREFIX` | Prefix for firmware repositories | Required |
| `--registry-username` | `REGISTRY_USERNAME` | Registry username | Required |
| `--registry-password` | `REGISTRY_PASSWORD` | Registry password | Required |
| `--registry-insecure` | `REGISTRY_INSECURE` | Use HTTP instead of HTTPS | `false` |
| `--cosign-pub-key-path` | `COSIGN_PUB_KEY_PATH` | Path to Cosign public key for verification | Optional |
| `--listen-addr` | `LISTEN_ADDR` | HTTP server listen address | `0.0.0.0:8080` |
| `--metrics-listen-addr` | `METRICS_LISTEN_ADDR` | Metrics server listen address | `0.0.0.0:9090` |
| `--log-level` | `LOG_LEVEL` | Log level (trace, debug, info, warn, error) | `info` |
| `--mqtt-url` | `MQTT_URL` | MQTT broker URL (enables MQTT notifications) | Optional |
| `--mqtt-username` | `MQTT_USERNAME` | MQTT username | Empty |
| `--mqtt-password` | `MQTT_PASSWORD` | MQTT password | Empty |
| `--mqtt-topic` | `MQTT_TOPIC` | MQTT topic prefix for notifications | Empty |
| `--mqtt-ca-cert-path` | `MQTT_CA_CERT_PATH` | Path to MQTT CA certificate (enables TLS) | Optional |
| `--mqtt-client-cert-path` | `MQTT_CLIENT_CERT_PATH` | Path to MQTT client certificate | Optional |
| `--mqtt-client-key-path` | `MQTT_CLIENT_KEY_PATH` | Path to MQTT client key | Optional |

## HTTP API

### Device Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/version?device=<device id>` | GET | Returns latest version, CRC32, and firmware size |
| `/firmware?device=<device id>` | GET | Serves the firmware binary |

Example response for `/version`:

```
0.1.1
4051932293
942320
```

### Webhook Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/webhooks/harbor` | POST | Receives Harbor registry webhook events |

### Metrics Endpoint

Prometheus metrics are exposed on a separate port (default: 9090) at `/metrics`.
