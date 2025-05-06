use anyhow::{anyhow, Context, Result};
use crc32fast;
use flate2::read::GzDecoder;
use parking_lot::Mutex;
use semver::Version;
use std::{collections::HashMap, io::Read, sync::Arc};
use tar::Archive;
use tracing::{debug, info, warn};
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::{config::AppConfig, firmware::models::FirmwareInfo, registry::client::RegistryClient};

const FIRMWARE_FILENAME: &str = "firmware.bin";

pub struct FirmwareManager {
    cache: Mutex<HashMap<String, Arc<FirmwareInfo>>>,
    client: Arc<RegistryClient>,
}

impl FirmwareManager {
    pub fn new(config: &AppConfig) -> Self {
        let client = Arc::new(RegistryClient::new(
            config.registry_url.clone(),
            config.repository_prefix.clone(),
            config.registry_username.clone(),
            config.registry_password.clone(),
            Arc::new(config.http_client.clone()),
        ));

        Self {
            cache: Mutex::new(Default::default()),
            client,
        }
    }

    /// Alias for backward compatibility with existing endpoints
    pub async fn get_current_firmware_for_device(
        &self,
        device_id: &str,
    ) -> Option<Arc<FirmwareInfo>> {
        self.get_firmware(device_id).await
    }

    pub async fn get_firmware(&self, device_id: &str) -> Option<Arc<FirmwareInfo>> {
        if let Some(info) = self.cache.lock().get(device_id) {
            debug!("Cache hit for {}", device_id);
            return Some(Arc::clone(info));
        }
        debug!("Cache miss for {}", device_id);

        match self.update(device_id, false).await {
            Ok(Some(info)) => Some(info),
            Ok(None) => self.cache.lock().get(device_id).cloned(),
            Err(e) => {
                warn!("Failed to update {}: {}", device_id, e);
                None
            }
        }
    }

    pub async fn update(&self, device_id: &str, force: bool) -> Result<Option<Arc<FirmwareInfo>>> {
        info!("Updating {} (force={})", device_id, force);

        let tags = self
            .client
            .fetch_tags(device_id)
            .await
            .with_context(|| format!("fetch_tags {}", device_id))?;

        let latest_tag = tags
            .iter()
            .filter_map(|t| Version::parse(t).ok().map(|v| (v, t)))
            .max_by_key(|(v, _)| v.clone())
            .map(|(_, t)| t.clone());

        let latest_tag = match latest_tag {
            Some(t) => t,
            None => {
                warn!("No semver tag for {}", device_id);
                return Ok(self.cache.lock().get(device_id).cloned());
            }
        };

        let latest_version = Version::parse(&latest_tag)
            .with_context(|| format!("parse version {} for {}", latest_tag, device_id))?;

        let should_update = force
            || self
                .cache
                .lock()
                .get(device_id)
                .map(|info| latest_version > info.version)
                .unwrap_or(true);

        if !should_update {
            debug!("{} is up-to-date", device_id);
            return Ok(self.cache.lock().get(device_id).cloned());
        }

        let blob = self
            .client
            .fetch_firmware(device_id, &latest_tag)
            .await
            .with_context(|| format!("fetch_firmware {}:{}", device_id, latest_tag))?;
        info!("Downloaded {} bytes", blob.len());

        let firmware_bytes = extract_firmware(&blob)
            .with_context(|| format!("extract {} for {}", latest_tag, device_id))?;

        let crc = crc32fast::hash(&firmware_bytes);
        let info = Arc::new(FirmwareInfo {
            version: latest_version.clone(),
            size: firmware_bytes.len(),
            crc,
            binary: firmware_bytes,
        });

        self.cache
            .lock()
            .insert(device_id.to_string(), Arc::clone(&info));
        debug!("Cached {}@{}", device_id, info.version);

        Ok(Some(info))
    }
}

fn extract_firmware(data: &[u8]) -> Result<Vec<u8>> {
    let is_gzip = data.starts_with(&[0x1F, 0x8B]);
    let is_zstd = data.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]);
    let is_tar = data.get(257..262) == Some(b"ustar");

    if !(is_gzip || is_zstd || is_tar) {
        return Ok(data.to_vec());
    }

    let reader: Box<dyn Read> = if is_gzip {
        Box::new(GzDecoder::new(data))
    } else if is_zstd {
        Box::new(ZstdDecoder::new(data)?)
    } else {
        Box::new(data)
    };

    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.ends_with(FIRMWARE_FILENAME) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }

    Err(anyhow!("{} not found in archive", FIRMWARE_FILENAME))
}
