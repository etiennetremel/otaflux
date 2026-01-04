use anyhow::{anyhow, Context, Result};
use oci_client::{
    client::{Client, ClientConfig, ClientProtocol},
    manifest::{
        OciManifest,
        OciManifest::{Image, ImageIndex},
    },
    secrets::RegistryAuth,
    Reference,
};
use serde::Deserialize;
use sigstore::cosign::client::Client as CosignClient;
use sigstore::cosign::CosignCapabilities;
use std::fs;
use tracing::{debug, error, info, instrument};

const COSIGN_SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";

#[derive(Deserialize, Debug)]
struct CosignSignedPayload {
    critical: CriticalSection,
}

#[derive(Deserialize, Debug)]
struct CriticalSection {
    image: ImageSection,
}

#[derive(Deserialize, Debug)]
struct ImageSection {
    #[serde(rename = "docker-manifest-digest")]
    #[serde(alias = "Docker-manifest-digest")]
    docker_manifest_digest: String,
}

/// Result of fetching a firmware blob from the registry.
#[derive(Debug)]
pub struct FetchBlobResult {
    /// The raw firmware binary data.
    pub data: Vec<u8>,
    /// The manifest digest (e.g., "sha256:abc123...") that uniquely identifies
    /// the artifact version. Used for cache invalidation when an artifact is
    /// rebuilt with the same semver tag.
    pub manifest_digest: String,
}

#[derive(Clone)]
pub struct RegistryClient {
    client: Client,
    auth: RegistryAuth,
    registry: String,
    cosign_pub_key: Option<String>,
}

impl RegistryClient {
    /// Creates a new registry client for fetching firmware from OCI registries.
    ///
    /// # Errors
    ///
    /// Returns an error if the cosign public key file cannot be read.
    pub fn new(
        registry: String,
        username: String,
        password: String,
        insecure: bool,
        cosign_pub_key_path: Option<String>,
    ) -> Result<Self> {
        let config = ClientConfig {
            protocol: if insecure {
                ClientProtocol::Http
            } else {
                ClientProtocol::Https
            },
            ..Default::default()
        };

        let client = Client::new(config);
        let auth = RegistryAuth::Basic(username, password);

        let cosign_pub_key = cosign_pub_key_path
            .map(|path| {
                fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read cosign public key from {path}"))
            })
            .transpose()?;

        if cosign_pub_key.is_some() {
            info!("Cosign signature verification enabled");
        }

        Ok(RegistryClient {
            client,
            auth,
            registry,
            cosign_pub_key,
        })
    }

    /// Fetches all available tags for a given repository from the registry.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry request fails or the image path is invalid.
    #[instrument(skip(self), fields(repository = %repository))]
    pub async fn fetch_tags(&self, repository: &str) -> Result<Vec<String>> {
        let image_ref = self.image_path(repository, None)?;
        debug!("Fetching tags for image repository");

        let tags_response = self
            .client
            .list_tags(&image_ref, &self.auth, None, None)
            .await?;

        Ok(tags_response.tags)
    }

    /// Fetches the manifest digest for a given repository and tag without downloading the blob.
    ///
    /// This is a lightweight operation used to check if the cached firmware is still valid
    /// by comparing digests.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be pulled from the registry.
    #[instrument(skip(self), fields(repository = %repository, tag = %tag))]
    pub async fn fetch_manifest_digest(&self, repository: &str, tag: &str) -> Result<String> {
        let image_ref = self.image_path(repository, Some(tag))?;
        let (_, digest) = self.client.pull_manifest(&image_ref, &self.auth).await?;
        Ok(digest)
    }

    /// Fetches a firmware blob from the registry for a given repository and tag.
    ///
    /// If cosign verification is enabled, validates the artifact signature before returning.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The manifest cannot be pulled
    /// - Cosign signature verification fails (when enabled)
    /// - The blob cannot be fetched
    #[instrument(skip(self), fields(repository = %repository, tag = %tag))]
    pub async fn fetch_blob(&self, repository: &str, tag: &str) -> Result<FetchBlobResult> {
        let artifact_image_ref = self.image_path(repository, Some(tag))?;
        let (_artifact_manifest, artifact_manifest_digest) = self
            .client
            .pull_manifest(&artifact_image_ref, &self.auth)
            .await?;

        let artifact_manifest_digest_str = artifact_manifest_digest.clone();

        if self.cosign_pub_key.is_some() {
            debug!("Verifying cosign signature");
            let signature_lookup_digest = artifact_manifest_digest_str
                .strip_prefix("sha256:")
                .unwrap_or(&artifact_manifest_digest_str);
            let signature_tag = format!("sha256-{signature_lookup_digest}.sig");

            let (cosign_payload_bytes, signature_base64) = self
                .fetch_cosign_signature_data(repository, &signature_tag)
                .await?;

            debug!(
                signature_len = signature_base64.len(),
                payload_len = cosign_payload_bytes.len(),
                "Fetched cosign signature data"
            );

            self.verify_cosign_signature(&cosign_payload_bytes, &signature_base64)?;

            let cosign_payload: CosignSignedPayload = serde_json::from_slice(&cosign_payload_bytes)
                .with_context(|| {
                    format!(
                        "Failed to deserialize Cosign signature payload for artifact {}. Payload: {}",
                        artifact_image_ref,
                        String::from_utf8_lossy(&cosign_payload_bytes)
                    )
                })?;

            if cosign_payload.critical.image.docker_manifest_digest != artifact_manifest_digest_str
            {
                return Err(anyhow!(
                    "Cosign signature payload verification failed for artifact {}: \
                digest mismatch. Expected '{}', got '{}' in payload.",
                    artifact_image_ref,
                    artifact_manifest_digest_str,
                    cosign_payload.critical.image.docker_manifest_digest
                ));
            }
            info!("Cosign payload verified and matches artifact digest");
        }

        let data = self
            .fetch_layer_blob(&artifact_image_ref, repository)
            .await?;

        Ok(FetchBlobResult {
            data,
            manifest_digest: artifact_manifest_digest_str,
        })
    }

    /// Fetches the cosign signature data for a given repository and signature tag.
    ///
    /// Returns a tuple of (signature payload bytes, base64-encoded signature string).
    /// The payload is typically a JSON document (Simple Signing format).
    async fn fetch_cosign_signature_data(
        &self,
        repository: &str,
        signature_tag: &str,
    ) -> Result<(Vec<u8>, String)> {
        let signature_image_ref = self.image_path(repository, Some(signature_tag))?;

        let (manifest, _) = self
            .client
            .pull_manifest(&signature_image_ref, &self.auth)
            .await?;

        let OciManifest::Image(signature_image_manifest) = manifest else {
            return Err(anyhow!(
                "Signature manifest for {signature_image_ref} is not an image manifest"
            ));
        };

        let signature_payload_layer = signature_image_manifest.layers.first().ok_or_else(|| {
            anyhow!(
                "Signature image {signature_image_ref} has no layers (expected signature payload)"
            )
        })?;

        let signature_base64 = signature_payload_layer
            .annotations
            .as_ref()
            .and_then(|a| a.get(COSIGN_SIGNATURE_ANNOTATION))
            .ok_or_else(|| {
                anyhow!(
                    "No '{COSIGN_SIGNATURE_ANNOTATION}' annotation found in the signature layer for {signature_image_ref}"
                )
            })?
            .clone();

        let mut signature_payload_bytes = Vec::new();
        self.client
            .pull_blob(
                &signature_image_ref,
                signature_payload_layer,
                &mut signature_payload_bytes,
            )
            .await?;

        if signature_payload_bytes.is_empty() {
            return Err(anyhow!(
                "Signature payload blob for {signature_image_ref} is empty"
            ));
        }

        Ok((signature_payload_bytes, signature_base64))
    }

    /// Fetches the actual artifact blob (firmware binary) from the first layer of the image.
    async fn fetch_layer_blob(
        &self,
        image_ref: &Reference,
        repository_name_for_error: &str,
    ) -> Result<Vec<u8>> {
        debug!(image = %image_ref, "Fetching artifact blob");

        let (manifest, _) = self.client.pull_manifest(image_ref, &self.auth).await?;

        let image_manifest = match manifest {
            ImageIndex(index) => {
                let first_manifest_descriptor = index
                    .manifests
                    .first()
                    .ok_or_else(|| anyhow!("Image index for {image_ref} is empty"))?;

                let platform_specific_image_ref = self.image_path(
                    repository_name_for_error,
                    Some(&first_manifest_descriptor.digest),
                )?;

                let (resolved_manifest, _resolved_digest) = self
                    .client
                    .pull_manifest(&platform_specific_image_ref, &self.auth)
                    .await?;

                match resolved_manifest {
                    OciManifest::Image(m) => m,
                    OciManifest::ImageIndex(_) => {
                        return Err(anyhow!(
                            "Resolved manifest for {platform_specific_image_ref} (from index) is not an ImageManifest"
                        ))
                    }
                }
            }
            Image(m) => m,
        };

        let artifact_layer_descriptor = image_manifest
            .layers
            .first()
            .ok_or_else(|| anyhow!("Image manifest for {image_ref} has no layers"))?;

        info!(
            digest = %artifact_layer_descriptor.digest,
            "Found artifact blob"
        );

        let mut blob_data: Vec<u8> = Vec::new();
        self.client
            .pull_blob(image_ref, artifact_layer_descriptor, &mut blob_data)
            .await?;

        if blob_data.is_empty() {
            Err(anyhow!("Fetched artifact blob for {image_ref} is empty"))
        } else {
            Ok(blob_data)
        }
    }

    /// Constructs a full OCI image reference string (e.g., "registry/repository:tag").
    fn image_path(&self, repository: &str, tag: Option<&str>) -> Result<Reference> {
        let reference_string = if let Some(tag_str) = tag {
            format!("{}/{}:{}", self.registry, repository, tag_str)
        } else {
            format!("{}/{}", self.registry, repository)
        };

        reference_string
            .parse()
            .with_context(|| format!("Invalid image reference: {reference_string}"))
    }

    /// Verifies a Cosign signature against a payload using the configured public key.
    ///
    /// # Arguments
    /// * `signed_payload_content` - The raw bytes of the content that was signed (Cosign Simple Signing JSON).
    /// * `signature_base64` - The base64-encoded signature string.
    fn verify_cosign_signature(
        &self,
        signed_payload_content: &[u8],
        signature_base64: &str,
    ) -> Result<()> {
        let pem_content = self.cosign_pub_key.as_ref().ok_or_else(|| {
            anyhow!("Cosign public key is not configured. Cannot verify signature.")
        })?;

        CosignClient::verify_blob_with_public_key(
            pem_content.trim(),
            signature_base64.trim(),
            signed_payload_content,
        )
        .map_err(|e| {
            error!(error = ?e, "Cosign cryptographic verification failed");
            anyhow!("Cosign signature verification failed")
        })?;

        info!("Cosign signature cryptographically verified");
        Ok(())
    }
}
