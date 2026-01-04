use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde::Serialize;
use tracing::{info, instrument, warn};

use crate::api::router::AppState;

#[derive(Debug, Deserialize)]
pub struct HarborWebhookPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    pub occur_at: u64,
    pub operator: String,
    pub event_data: HarborEventData,
}

#[derive(Debug, Deserialize)]
pub struct HarborEventData {
    pub resources: Vec<HarborResource>,
    pub repository: HarborRepository,
}

#[derive(Debug, Deserialize)]
pub struct HarborResource {
    pub digest: String,
    pub tag: String,
    pub resource_url: String,
}

#[derive(Debug, Deserialize)]
pub struct HarborRepository {
    pub date_created: u64,
    pub name: String,
    pub namespace: String,
    pub repo_full_name: String,
    pub repo_type: String,
}

#[derive(Serialize)]
pub struct FirmwarePayload {
    version: String,
    size: usize,
}

#[instrument(skip(app, payload), fields(event_type = %payload.event_type, operator = %payload.operator))]
pub async fn harbor_webhook_handler(
    State(app): State<AppState>,
    Json(payload): Json<HarborWebhookPayload>,
) -> impl IntoResponse {
    info!("Received Harbor webhook");

    if payload.event_type != "PUSH_ARTIFACT" {
        warn!(event_type = %payload.event_type, "Ignoring non-push event");
        return StatusCode::OK;
    }

    let device_id = &payload.event_data.repository.name;

    for resource in &payload.event_data.resources {
        info!(
            device_id = %device_id,
            tag = %resource.tag,
            "Processing PUSH_ARTIFACT event"
        );

        match app.firmware_manager.get_firmware(device_id).await {
            Ok(fw) => {
                let payload_data = FirmwarePayload {
                    version: fw.version.to_string(),
                    size: fw.size,
                };

                match serde_json::to_vec(&payload_data) {
                    Ok(payload_bytes) => {
                        if let Some(notifier) = &app.notifier {
                            match notifier.publish(device_id.clone(), payload_bytes).await {
                                Ok(()) => {
                                    info!(
                                        device_id = %device_id,
                                        tag = %resource.tag,
                                        "Published firmware notification"
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        device_id = %device_id,
                                        tag = %resource.tag,
                                        error = ?e,
                                        "Failed to publish MQTT notification"
                                    );
                                }
                            }
                        } else {
                            warn!("No notifier configured, skipping MQTT notification");
                        }
                    }
                    Err(e) => {
                        warn!(
                            device_id = %device_id,
                            tag = %resource.tag,
                            error = ?e,
                            "Failed to serialize firmware payload"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    device_id = %device_id,
                    tag = %resource.tag,
                    error = ?e,
                    "Failed to get firmware"
                );
            }
        }
    }

    StatusCode::OK
}
