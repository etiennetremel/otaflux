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
use serde::{Deserialize, Serialize};
use sigstore::cosign::client::Client as CosignClient;
use sigstore::cosign::CosignCapabilities;
use std::fs;
use tracing::{debug, error, info};

const COSIGN_SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";

// Structs for deserializing the Cosign Simple Signing JSON payload
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
    #[serde(alias = "Docker-manifest-digest")] // GCR uses different casing
    docker_manifest_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RegistryTagList {
    pub name: String,
    pub tags: Vec<String>,
}

/// Client for interacting with an OCI registry, including Cosign signature verification.
#[derive(Clone)]
pub struct RegistryClient {
    client: Client,
    auth: RegistryAuth,
    registry: String,
    cosign_pub_key_path: Option<String>,
}

impl RegistryClient {
    /// Creates a new `RegistryClient`.
    ///
    /// # Arguments
    /// * `registry` - The base URL of the OCI registry (e.g., "my.registry.com").
    /// * `username` - Username for registry authentication.
    /// * `password` - Password for registry authentication.
    /// * `insecure` - If true, use HTTP; otherwise, use HTTPS.
    /// * `cosign_pub_key_path` - Optional path to the Cosign public key file for signature verification.
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

        Ok(RegistryClient {
            client,
            auth,
            registry,
            cosign_pub_key_path,
        })
    }

    /// Fetches a list of tags for a given repository.
    pub async fn fetch_tags(&self, repository: &str) -> Result<Vec<String>> {
        let image_ref = self.image_path(repository, None)?;
        debug!("Fetching tags for image repository: {}", image_ref);

        let tags_response = self
            .client
            .list_tags(&image_ref, &self.auth, None, None)
            .await?;

        Ok(tags_response.tags)
    }

    /// Fetches an artifact blob from the registry and verifies its Cosign signature if a cosign pub key is provided.
    ///
    /// This involves:
    /// 1. Fetching the artifact's manifest to get its digest.
    /// 2. Constructing the Cosign signature tag (e.g., `sha256-<digest>.sig`).
    /// 3. Fetching the Cosign signature payload and the base64-encoded signature itself.
    /// 4. Cryptographically verifying the signature against the Cosign payload using the configured public key.
    /// 5. Deserializing the verified Cosign payload and ensuring it references the correct artifact manifest digest.
    /// 6. Fetching the actual artifact blob (first layer of the artifact image).
    pub async fn fetch_blob(&self, repository: &str, tag: &str) -> Result<Vec<u8>> {
        // 1. Fetch the artifact's manifest to get its digest.
        //    This manifest_digest is what Cosign's SimpleSigning payload should refer to.
        let artifact_image_ref = self.image_path(repository, Some(tag))?;
        let (_artifact_manifest, artifact_manifest_digest) = self
            .client
            .pull_manifest(&artifact_image_ref, &self.auth)
            .await?;

        let artifact_manifest_digest_str = artifact_manifest_digest.to_string();

        if self.cosign_pub_key_path.is_some() {
            debug!("Verifying cosign signature...");
            // 2. Construct the Cosign signature tag.
            let signature_lookup_digest = artifact_manifest_digest_str
                .strip_prefix("sha256:")
                .unwrap_or(&artifact_manifest_digest_str);
            let signature_tag = format!("sha256-{}.sig", signature_lookup_digest);

            // 3. Fetch the Cosign signature payload and the base64-encoded signature.
            //    The `cosign_payload_bytes` is the JSON data that was actually signed.
            let (cosign_payload_bytes, signature_base64) = self
                .fetch_cosign_signature_data(repository, &signature_tag)
                .await?;

            debug!("Cosign signature (base64): {}", signature_base64);
            debug!(
                "Cosign payload (bytes length): {}",
                cosign_payload_bytes.len()
            );

            // 4. Cryptographically verify the signature against the Cosign payload.
            self.verify_cosign_signature(cosign_payload_bytes.clone(), signature_base64)?;

            // 5. Deserialize the verified Cosign payload and check its integrity.
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
            info!(
                "Cosign payload successfully verified and matches artifact digest for {}",
                artifact_image_ref
            );
        }

        // 6. Fetch the actual artifact blob.
        self.fetch_layer_blob(&artifact_image_ref, repository).await
    }

    /// Fetches the Cosign signature data (payload and base64 signature string).
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

        let signature_image_manifest = match manifest {
            Image(m) => m,
            _ => {
                return Err(anyhow!(
                    "Signature manifest for {} is not an image manifest",
                    signature_image_ref
                ))
            }
        };

        let signature_payload_layer = signature_image_manifest.layers.first().ok_or_else(|| {
            anyhow!(
                "Signature image {} has no layers (expected signature payload)",
                signature_image_ref
            )
        })?;

        // Extract base64 signature from layer annotations
        let signature_base64 = signature_payload_layer
            .annotations
            .as_ref()
            .and_then(|a| a.get(COSIGN_SIGNATURE_ANNOTATION))
            .ok_or_else(|| {
                anyhow!(
                    "No '{}' annotation found in the signature layer for {}",
                    COSIGN_SIGNATURE_ANNOTATION,
                    signature_image_ref
                )
            })?
            .to_string();

        // The layer itself is the signature payload (e.g., Simple Signing JSON)
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
                "Signature payload blob for {} is empty",
                signature_image_ref
            ));
        }

        Ok((signature_payload_bytes, signature_base64))
    }

    /// Fetches the actual artifact blob, typically the first layer of the specified image.
    async fn fetch_layer_blob(
        &self,
        image_ref: &Reference,
        repository_name_for_error: &str,
    ) -> Result<Vec<u8>> {
        debug!("Fetching artifact blob for image: {}", image_ref);

        let (manifest, _) = self.client.pull_manifest(image_ref, &self.auth).await?;

        let image_manifest = match manifest {
            ImageIndex(index) => {
                let first_manifest_descriptor = index
                    .manifests
                    .first()
                    .ok_or_else(|| anyhow!("Image index for {} is empty", image_ref))?;

                // Construct reference for the specific image manifest within the index
                let platform_specific_image_ref = self.image_path(
                    repository_name_for_error,               // Use original repo name
                    Some(&first_manifest_descriptor.digest), // and digest for tag
                )?;

                let (resolved_manifest, _resolved_digest) = self
                    .client
                    .pull_manifest(&platform_specific_image_ref, &self.auth)
                    .await?;

                match resolved_manifest {
                    OciManifest::Image(m) => m,
                    _ => {
                        return Err(anyhow!(
                            "Resolved manifest for {} (from index) is not an ImageManifest",
                            platform_specific_image_ref
                        ))
                    }
                }
            }
            Image(m) => m,
        };

        let artifact_layer_descriptor = image_manifest
            .layers
            .first()
            .ok_or_else(|| anyhow!("Image manifest for {} has no layers", image_ref))?;

        info!(
            "Artifact blob digest for {}: {:?}",
            image_ref, artifact_layer_descriptor.digest
        );

        let mut blob_data: Vec<u8> = Vec::new();
        self.client
            .pull_blob(image_ref, artifact_layer_descriptor, &mut blob_data)
            .await?;

        if blob_data.is_empty() {
            Err(anyhow!("Fetched artifact blob for {} is empty", image_ref))
        } else {
            Ok(blob_data)
        }
    }

    /// Constructs a full image reference string (e.g., "registry/repository:tag").
    fn image_path(&self, repository: &str, tag: Option<&str>) -> Result<Reference> {
        let reference_string = if let Some(tag_str) = tag {
            format!("{}/{}:{}", self.registry, repository, tag_str)
        } else {
            format!("{}/{}", self.registry, repository) // For listing tags, no tag is specified
        };

        reference_string
            .parse()
            .with_context(|| format!("Invalid image reference: {}", reference_string))
    }

    /// Verifies a Cosign signature against a payload using a PEM-encoded public key.
    ///
    /// # Arguments
    /// * `signed_payload_content` - The raw bytes of the content that was signed (e.g., Cosign Simple Signing JSON).
    /// * `signature_base64` - The base64-encoded signature string.
    ///
    /// Note: This function assumes `CosignClient::verify_blob_with_public_key` from your
    /// `sigstore-rs` version accepts a PEM string as its first argument. If you update
    /// `sigstore-rs`, you might need to parse the PEM into a `PublicKey` object first.
    fn verify_cosign_signature(
        &self,
        signed_payload_content: Vec<u8>,
        signature_base64: String,
    ) -> Result<()> {
        let pubkey_path_str = self.cosign_pub_key_path.as_ref().ok_or_else(|| {
            anyhow!("Cosign public key path is not configured. Cannot verify signature.")
        })?;

        let pem_content = fs::read_to_string(pubkey_path_str)?;

        // This call assumes your version of CosignClient::verify_blob_with_public_key
        // takes a PEM string as the first argument.
        CosignClient::verify_blob_with_public_key(
            pem_content.trim(),
            signature_base64.trim(),
            &signed_payload_content,
        )
        .map_err(|e| {
            // Log the detailed error from sigstore library for better debugging
            error!(
                "Cosign cryptographic verification failed for key {}. Raw error: {:?}",
                pubkey_path_str, e
            );
            anyhow!(
                "Cosign signature verification failed using public key '{}'",
                pubkey_path_str
            )
        })?;

        info!(
            "Cosign signature cryptographically verified successfully using key {}",
            pubkey_path_str
        );
        Ok(())
    }
}
