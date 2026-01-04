use anyhow::{anyhow, Result};
use bytes::Bytes;
use lru::LruCache;
use parking_lot::Mutex;
use semver::Version;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, instrument, warn};

use crate::registry::RegistryClient;

/// Default maximum number of firmware entries to cache.
const DEFAULT_CACHE_SIZE: usize = 100;

#[derive(Clone, Debug)]
pub struct FirmwareInfo {
    pub binary: Bytes,
    pub crc: u32,
    pub version: Version,
    pub size: usize,
    /// The manifest digest from the registry, used to detect rebuilt artifacts
    /// with the same semver tag.
    pub manifest_digest: String,
}

struct CacheState {
    entries: LruCache<String, Arc<FirmwareInfo>>,
    /// Tracks device IDs currently being fetched to prevent thundering herd.
    in_flight: HashSet<String>,
}

pub struct FirmwareManager {
    cache: Mutex<CacheState>,
    client: Arc<RegistryClient>,
    /// Channel to notify waiting requests when a fetch completes.
    fetch_complete_tx: broadcast::Sender<String>,
}

impl FirmwareManager {
    /// Creates a new instance of `FirmwareManager` with default cache size.
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
        Self::with_cache_size(
            url,
            username,
            password,
            insecure,
            prefix,
            cosign_pub_key_path,
            DEFAULT_CACHE_SIZE,
        )
    }

    /// Creates a new instance of `FirmwareManager` with a custom cache size.
    ///
    /// # Arguments
    ///
    /// * `url` - The base URL of the OCI registry.
    /// * `username` - The username for registry authentication.
    /// * `password` - The password for registry authentication.
    /// * `insecure` - A boolean indicating whether to allow insecure connections to the registry.
    /// * `prefix` - The repository prefix to use within the registry.
    /// * `cosign_pub_key_path` - An optional path to a cosign public key for signature verification.
    /// * `cache_size` - Maximum number of firmware entries to cache.
    ///
    /// # Returns
    ///
    /// A `Result` containing the new `FirmwareManager` instance or an error if initialization fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the `RegistryClient` fails to initialize or `cache_size` is 0.
    pub fn with_cache_size(
        url: String,
        username: String,
        password: String,
        insecure: bool,
        prefix: &str,
        cosign_pub_key_path: Option<String>,
        cache_size: usize,
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

        let cache_capacity = NonZeroUsize::new(cache_size)
            .ok_or_else(|| anyhow!("Cache size must be greater than 0"))?;

        let (fetch_complete_tx, _) = broadcast::channel(16);

        Ok(Self {
            cache: Mutex::new(CacheState {
                entries: LruCache::new(cache_capacity),
                in_flight: HashSet::new(),
            }),
            client,
            fetch_complete_tx,
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
    #[instrument(skip(self), fields(device_id = %device_id))]
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
            return Err(anyhow!("No semver tag found for {device_id}"));
        };

        let latest_version = Version::parse(&latest_tag)
            .map_err(|e| anyhow!("Couldn't parse version from tag '{latest_tag}': {e}"))?;

        Ok((latest_tag, latest_version))
    }

    /// Updates the cache size metric gauge.
    #[allow(clippy::unused_self)]
    fn update_cache_size_metric(&self, cache: &CacheState) {
        #[allow(clippy::cast_precision_loss)]
        metrics::gauge!("firmware_cache_entries").set(cache.entries.len() as f64);
    }

    /// Records a cache hit metric for the given device.
    #[allow(clippy::unused_self)]
    fn record_cache_hit(&self, device_id: &str) {
        metrics::counter!("firmware_cache_hit_total", "device_id" => device_id.to_string())
            .increment(1);
    }

    /// Records a cache miss metric for the given device.
    #[allow(clippy::unused_self)]
    fn record_cache_miss(&self, device_id: &str) {
        metrics::counter!("firmware_cache_miss_total", "device_id" => device_id.to_string())
            .increment(1);
    }

    /// Retrieves the latest firmware for the specified device.
    ///
    /// This method checks the cache for the latest firmware version for the given device ID.
    /// If the cached version is outdated or missing, it fetches the latest firmware from the registry,
    /// updates the cache, and returns the firmware information.
    ///
    /// The method implements thundering herd protection: if multiple requests arrive for the same
    /// device simultaneously, only one will fetch from the registry while others wait.
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
    #[instrument(skip(self), fields(device_id = %device_id))]
    pub async fn get_firmware(&self, device_id: &str) -> Result<Arc<FirmwareInfo>> {
        debug!("Fetching firmware for device");

        let (latest_tag, latest_version) = self.get_latest_version(device_id).await?;
        info!(version = %latest_version, "Found latest version for device");

        // Fetch manifest digest to detect rebuilt artifacts with same version
        let current_digest = self
            .client
            .fetch_manifest_digest(device_id, &latest_tag)
            .await?;

        // Check cache and handle in-flight requests (thundering herd protection)
        let should_fetch = {
            let mut cache = self.cache.lock();
            self.update_cache_size_metric(&cache);

            if let Some(cached_firmware) = cache.entries.get(device_id) {
                // Cache hit: check if version AND digest match (digest detects rebuilt artifacts)
                if latest_version <= cached_firmware.version
                    && current_digest == cached_firmware.manifest_digest
                {
                    debug!(
                        version = %latest_version,
                        digest = %current_digest,
                        "Cache hit - firmware is up-to-date"
                    );
                    self.record_cache_hit(device_id);
                    return Ok(Arc::clone(cached_firmware));
                }
                debug!(
                    cached_version = %cached_firmware.version,
                    cached_digest = %cached_firmware.manifest_digest,
                    latest_version = %latest_version,
                    current_digest = %current_digest,
                    "Cache stale - newer version or different digest"
                );
            }

            // Check if another request is already fetching this device
            if cache.in_flight.contains(device_id) {
                debug!("Another request is fetching firmware, waiting...");
                false
            } else {
                cache.in_flight.insert(device_id.to_string());
                true
            }
        };

        if !should_fetch {
            // Wait for the in-flight request to complete
            let mut rx = self.fetch_complete_tx.subscribe();
            loop {
                match rx.recv().await {
                    Ok(completed_device) if completed_device == device_id => {
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        // Channel closed or lagged, try to get from cache anyway
                        break;
                    }
                }
            }

            // Check cache again after waiting
            let cache = self.cache.lock();
            if let Some(cached_firmware) = cache.entries.peek(device_id) {
                debug!("Got firmware from cache after waiting");
                return Ok(Arc::clone(cached_firmware));
            }
            return Err(anyhow!(
                "Failed to get firmware after waiting for in-flight request"
            ));
        }

        // We're responsible for fetching - ensure we clean up in_flight on any exit path
        let result = self
            .fetch_and_cache_firmware(device_id, &latest_tag, latest_version)
            .await;

        // Clean up in_flight and notify waiters
        {
            let mut cache = self.cache.lock();
            cache.in_flight.remove(device_id);
            self.update_cache_size_metric(&cache);
        }
        let _ = self.fetch_complete_tx.send(device_id.to_string());

        result
    }

    /// Fetches firmware from the registry and caches it.
    async fn fetch_and_cache_firmware(
        &self,
        device_id: &str,
        latest_tag: &str,
        latest_version: Version,
    ) -> Result<Arc<FirmwareInfo>> {
        debug!("Cache miss - fetching from registry");
        self.record_cache_miss(device_id);

        // No lock is held here during the await
        let fetch_result = self.client.fetch_blob(device_id, latest_tag).await?;
        let blob_len = fetch_result.data.len();
        info!(bytes = blob_len, "Downloaded firmware");

        let firmware_bytes = Bytes::from(fetch_result.data);
        let crc = crc32fast::hash(&firmware_bytes);
        let info = Arc::new(FirmwareInfo {
            version: latest_version.clone(),
            size: blob_len,
            crc,
            binary: firmware_bytes,
            manifest_digest: fetch_result.manifest_digest,
        });

        // Reacquire the lock to update the cache
        {
            let mut cache = self.cache.lock();
            cache.entries.put(device_id.to_string(), Arc::clone(&info));
            self.update_cache_size_metric(&cache);
            debug!(version = %info.version, "Cached firmware");
        }

        Ok(info)
    }
}
