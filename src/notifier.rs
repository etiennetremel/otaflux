use anyhow::{anyhow, Result};
use rumqttc::EventLoop;
use rumqttc::{AsyncClient, MqttOptions, QoS, TlsConfiguration, Transport};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, instrument};

#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub ca_cert: Vec<u8>,
    pub client_auth: Option<(Vec<u8>, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct Notifier {
    client: Arc<AsyncClient>,
    topic: String,
}

impl Notifier {
    /// Creates a new MQTT notifier and returns both the notifier and its event loop.
    ///
    /// # Errors
    ///
    /// Returns an error if the MQTT URL is invalid or TLS configuration fails.
    pub fn new(
        url: String,
        username: String,
        password: String,
        topic: String,
        tls_config: Option<TlsConfig>,
    ) -> Result<(Self, EventLoop), anyhow::Error> {
        let mut mqttoptions = MqttOptions::parse_url(url)?;
        mqttoptions.set_keep_alive(Duration::from_secs(5));

        if !username.is_empty() {
            mqttoptions.set_credentials(username, password);
        }

        if let Some(tls) = tls_config {
            let transport = Transport::Tls(TlsConfiguration::Simple {
                ca: tls.ca_cert,
                alpn: None,
                client_auth: tls.client_auth,
            });
            mqttoptions.set_transport(transport);
        }

        let (client, eventloop) = AsyncClient::new(mqttoptions, 10);

        Ok((
            Self {
                client: Arc::new(client),
                topic,
            },
            eventloop,
        ))
    }

    /// Publishes a payload to the MQTT topic for a specific device.
    ///
    /// # Errors
    ///
    /// Returns an error if the MQTT client fails to publish the message.
    #[instrument(skip(self, payload), fields(topic = %format!("{}/{}", self.topic, device_id)))]
    pub async fn publish(&self, device_id: String, payload: Vec<u8>) -> Result<(), anyhow::Error> {
        let topic = format!("{}/{}", self.topic, device_id);
        info!(payload_size = payload.len(), "Publishing MQTT message");
        self.client
            .publish(topic.clone(), QoS::AtLeastOnce, true, payload)
            .await
            .map_err(|e| anyhow!("Failed to publish message to {topic:?}: {e:?}"))
    }
}
