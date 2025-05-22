use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use semver::Version;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, error, info, warn};

use crate::registry::RegistryClient;

#[derive(Clone, Debug)]
pub struct FirmwareInfo {
    pub binary: Vec<u8>,
    pub crc: u32,
    pub version: Version,
    pub size: usize,
}

pub struct FirmwareManager {
    cache: Mutex<HashMap<String, Arc<FirmwareInfo>>>,
    client: Arc<RegistryClient>,
}

impl FirmwareManager {
    /// Creates a new instance of `FirmwareManager`.
    ///
    /// # Arguments
    ///
    /// * `url` - The base URL of the OCI registry.
    /// * `username` - The username for registry authentication.
    /// * `password` - The password for registry authentication.
    /// * `insecure` - A boolean indicating whether to allow insecure connections to the registry.
    /// * `prefix` - The repository prefix to use within the registry.
    /// * `cosign_pub_key_path` - An optional path to a cosign public key for signature verification.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `FirmwareManager` instance or an error if initialization fails.
    pub fn new(
        url: String,
        username: String,
        password: String,
        insecure: bool,
        prefix: String,
        cosign_pub_key_path: Option<String>,
    ) -> Result<Self, anyhow::Error> {
        let repository = format!("{}/{}", url, prefix);
        let registry_client = RegistryClient::new(
            repository,
            username,
            password,
            insecure,
            cosign_pub_key_path,
        )?;

        let client = Arc::new(registry_client);

        Ok(Self {
            cache: Mutex::new(Default::default()),
            client,
        })
    }

    /// Retrieves firmware information for a given device ID.
    ///
    /// This method first checks the local cache for the firmware. If not found,
    /// it attempts to update the firmware from the registry.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device.
    ///
    /// # Returns
    ///
    /// An `Option` containing an `Arc<FirmwareInfo>` if firmware is found or successfully retrieved,
    /// otherwise `None`.
    pub async fn get_firmware(&self, device_id: &str) -> Option<Arc<FirmwareInfo>> {
        let labels = [("device_id", device_id.to_string())];

        if let Some(info) = self.cache.lock().get(device_id) {
            debug!("Cache hit for {}", device_id);
            metrics::counter!("firmware_cache_hit_total", &labels).increment(1);
            return Some(Arc::clone(info));
        }

        debug!("Cache miss for {}", device_id);
        metrics::counter!("firmware_cache_miss_total", &labels).increment(1);

        match self.update(device_id).await {
            Ok(Some(info)) => Some(info),
            Ok(None) => self.cache.lock().get(device_id).cloned(),
            Err(e) => {
                error!("Failed to update {}: {}", device_id, e);
                None
            }
        }
    }

    /// Fetches the latest semantic version tag for a given device ID from the registry.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device.
    ///
    /// # Returns
    ///
    /// A `Result` containing a tuple of the latest tag string and its parsed `Version`,
    /// or an error if no valid semantic version tag is found or parsing fails.
    async fn get_latest_version(&self, device_id: &str) -> Result<(String, Version)> {
        let tags = self.client.fetch_tags(device_id).await?;

        let latest_tag = tags
            .iter()
            .filter_map(|t| Version::parse(t).ok().map(|v| (v, t)))
            .max_by_key(|(v, _)| v.clone())
            .map(|(_, t)| t.clone());

        let latest_tag = match latest_tag {
            Some(t) => t,
            None => {
                warn!("No semver tag for {}", device_id);
                // Return an error to prevent further processing if no valid semver tag found
                return Err(anyhow!("No semver tag found for {}", device_id));
            }
        };

        let latest_version = Version::parse(&latest_tag)
            .map_err(|e| anyhow!("Couldn't parse version from tag '{}': {}", latest_tag, e))?;

        Ok((latest_tag, latest_version))
    }

    /// Updates the firmware for a given device ID by fetching the latest version from the registry.
    ///
    /// This method checks if an update is necessary by comparing the version in the cache
    /// with the latest version available in the registry. If an update is needed or the
    /// firmware is not in the cache, it downloads and caches the new firmware.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device.
    ///
    /// # Returns
    ///
    /// A `Result` containing an `Option<Arc<FirmwareInfo>>`.
    /// - `Ok(Some(info))` if the firmware was updated or fetched for the first time.
    /// - `Ok(None)` if the firmware is already up-to-date or if fetching the latest version failed but a cached version exists.
    /// - `Err(e)` if an error occurred during the update process (e.g., network issues, parsing errors) and no cached version was available.
    pub async fn update(&self, device_id: &str) -> Result<Option<Arc<FirmwareInfo>>> {
        info!("Updating {}", device_id);

        let (latest_tag, latest_version) = match self.get_latest_version(device_id).await {
            Ok(v) => v,
            // If get_latest_version fails, check cache. If it was already in cache, return that,
            // otherwise, return Ok(None) to signify no update.
            Err(e) => {
                warn!("Failed to get latest version for {}: {}", device_id, e);
                return Ok(self.cache.lock().get(device_id).cloned());
            }
        };

        info!("Latest version for {} is {}", device_id, latest_version);

        let should_update = self
            .cache
            .lock()
            .get(device_id)
            .map(|info| latest_version > info.version)
            .unwrap_or(true); // If not in cache, always update

        if !should_update {
            debug!("{} is up-to-date (version {})", device_id, latest_version);
            return Ok(self.cache.lock().get(device_id).cloned());
        }

        let blob = self.client.fetch_blob(device_id, &latest_tag).await?;
        info!("Downloaded {} bytes", blob.len());

        // --- SIMPLIFICATION: Assuming blob *is* the firmware binary ---
        let firmware_bytes = blob; // No extraction needed!

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
