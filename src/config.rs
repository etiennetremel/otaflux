use anyhow::{Context, Result};
use reqwest::Client;
use std::{env, time::Duration};

pub struct AppConfig {
    pub registry_url: String,
    pub repository_prefix: String,
    pub registry_username: Option<String>,
    pub registry_password: Option<String>,
    pub listen_addr: String,
    pub metrics_listen_addr: String,
    pub http_client: Client,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let registry_url = get_required("REGISTRY_URL")?;
        let repository_prefix = get_required("REPOSITORY_PREFIX")?;

        let registry_username = env::var("REGISTRY_USERNAME").ok();
        let registry_password = env::var("REGISTRY_PASSWORD").ok();

        let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
        let metrics_listen_addr =
            env::var("METRICS_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:9090".into());

        let http_client = Client::builder()
            .user_agent("otaflux")
            .timeout(Duration::from_secs(30))
            .build()
            .context("building HTTP client failed")?;

        Ok(Self {
            registry_url,
            repository_prefix,
            registry_username,
            registry_password,
            listen_addr,
            metrics_listen_addr,
            http_client,
        })
    }
}

fn get_required(key: &str) -> Result<String> {
    env::var(key).with_context(|| format!("{} environment variable must be set", key))
}
