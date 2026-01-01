use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use semver::Version;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, info, warn};

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
    ///
    /// # Errors
    ///
    /// Returns an error if the `RegistryClient` fails to initialize.
    pub fn new(
        url: String,
        username: String,
        password: String,
        insecure: bool,
        prefix: &str,
        cosign_pub_key_path: Option<String>,
    ) -> Result<Self, anyhow::Error> {
        // Build the registry string, avoiding double slashes when prefix is empty
        let repository = if prefix.is_empty() {
            url
        } else {
            format!("{url}/{prefix}")
        };
        let registry_client = RegistryClient::new(
            repository,
            username,
            password,
            insecure,
            cosign_pub_key_path,
        )?;

        let client = Arc::new(registry_client);

        Ok(Self {
            cache: Mutex::new(HashMap::default()),
            client,
        })
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

        let Some(latest_tag) = latest_tag else {
            warn!("No semver tag for {}", device_id);
            // Return an error to prevent further processing if no valid semver tag found
            return Err(anyhow!("No semver tag found for {}", device_id));
        };

        let latest_version = Version::parse(&latest_tag)
            .map_err(|e| anyhow!("Couldn't parse version from tag '{}': {}", latest_tag, e))?;

        Ok((latest_tag, latest_version))
    }

    /// Retrieves the latest firmware for the specified device.
    ///
    /// This method checks the cache for the latest firmware version for the given device ID.
    /// If the cached version is outdated or missing, it fetches the latest firmware from the registry,
    /// updates the cache, and returns the firmware information.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device.
    ///
    /// # Returns
    ///
    /// A `Result` containing an `Arc<FirmwareInfo>` with the latest firmware data, or an error if retrieval fails.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No valid semantic version tag is found for the device.
    /// - Fetching the firmware blob from the registry fails.
    pub async fn get_firmware(&self, device_id: &str) -> Result<Arc<FirmwareInfo>> {
        info!("Updating {}", device_id);

        let prometheus_labels = [("device_id", device_id.to_string())];

        let (latest_tag, latest_version) = self.get_latest_version(device_id).await?;
        info!("Latest version for {} is {}", device_id, latest_version);

        // Scope the lock to only check the cache and determine if an update is needed
        let current_firmware_in_cache = {
            let cache = self.cache.lock();
            cache.get(device_id).cloned()
        };

        // Return cached firmware if it's up-to-date
        if let Some(cached_firmware) = current_firmware_in_cache {
            if latest_version <= cached_firmware.version {
                debug!("{} is up-to-date (version {})", device_id, latest_version);
                metrics::counter!("firmware_cache_hit_total", &prometheus_labels).increment(1);
                return Ok(cached_firmware);
            }
        }

        debug!("Cache miss for {}", device_id);
        metrics::counter!("firmware_cache_miss_total", &prometheus_labels).increment(1);

        // No lock is held here during the await
        let blob = self.client.fetch_blob(device_id, &latest_tag).await?;
        info!("Downloaded {} bytes", blob.len());

        let firmware_bytes = blob;
        let crc = crc32fast::hash(&firmware_bytes);
        let info = Arc::new(FirmwareInfo {
            version: latest_version.clone(),
            size: firmware_bytes.len(),
            crc,
            binary: firmware_bytes,
        });

        // Reacquire the lock to update the cache
        {
            let mut cache = self.cache.lock();
            cache.insert(device_id.to_string(), Arc::clone(&info));
            debug!("Cached {}@{}", device_id, info.version);
        }

        Ok(info)
    }
}
