use anyhow::{anyhow, Context, Result};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use tracing::debug;

use crate::registry::models::RegistryTagList;

#[derive(Clone)]
pub struct RegistryClient {
    registry_url: String,
    base_repo: String,
    username: Option<String>,
    password: Option<String>,
    client: std::sync::Arc<Client>,
}

impl RegistryClient {
    pub fn new(
        registry_url: impl Into<String>,
        base_repo: impl Into<String>,
        username: Option<String>,
        password: Option<String>,
        client: std::sync::Arc<Client>,
    ) -> Self {
        RegistryClient {
            registry_url: registry_url.into(),
            base_repo: base_repo.into(),
            username,
            password,
            client,
        }
    }

    pub async fn fetch_tags(&self, device_id: &str) -> Result<Vec<String>> {
        let path = self.repo_path(device_id);
        let url = format!("{}/v2/{}/tags/list", self.registry_url, path);

        let resp = self
            .auth(self.client.get(&url))
            .send()
            .await
            .context("fetch_tags: request failed")?;

        let list: RegistryTagList = resp.json().await.context("fetch_tags: parse error")?;

        Ok(list.tags)
    }

    pub async fn fetch_firmware(&self, device_id: &str, tag: &str) -> Result<Vec<u8>> {
        let manifest = self.fetch_manifest(device_id, tag).await?;
        let digest = self.extract_blob_digest(&manifest)?;
        self.fetch_blob(device_id, digest).await
    }

    async fn fetch_manifest(&self, device_id: &str, tag: &str) -> Result<Value> {
        let path = self.repo_path(device_id);
        let accept = "application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.index.v1+json";
        let mut reference = tag.to_string();

        loop {
            let url = format!("{}/v2/{}/manifests/{}", self.registry_url, path, reference);
            let resp = self
                .auth(self.client.get(&url).header("Accept", accept))
                .send()
                .await
                .context("fetch_manifest: request failed")?;

            debug!(
                "Manifest content type: {:?}",
                resp.headers().get(reqwest::header::CONTENT_TYPE)
            );
            let manifest: Value = resp.json().await.context("fetch_manifest: parse error")?;

            if let Some(media) = manifest.get("mediaType").and_then(Value::as_str) {
                if media == "application/vnd.docker.distribution.manifest.list.v2+json"
                    || media == "application/vnd.oci.image.index.v1+json"
                {
                    reference = manifest["manifests"][0]["digest"]
                        .as_str()
                        .context("manifest list missing digest")?
                        .to_string();
                    continue;
                }
            }

            return Ok(manifest);
        }
    }

    fn extract_blob_digest<'a>(&self, m: &'a Value) -> Result<&'a str> {
        if let Some(layers) = m.get("layers") {
            layers[0]["digest"].as_str().context("no layer digest")
        } else if let Some(fs) = m.get("fsLayers") {
            fs[0]["blobSum"].as_str().context("no blobSum")
        } else {
            Err(anyhow!("unsupported manifest format"))
        }
    }

    async fn fetch_blob(&self, device_id: &str, digest: &str) -> Result<Vec<u8>> {
        let path = self.repo_path(device_id);
        let url = format!("{}/v2/{}/blobs/{}", self.registry_url, path, digest);

        let resp = self
            .auth(self.client.get(&url))
            .send()
            .await
            .context("fetch_blob: request failed")?;

        debug!(
            "Blob content type: {:?}",
            resp.headers().get(reqwest::header::CONTENT_TYPE)
        );
        let data = resp.bytes().await.context("blob read error")?.to_vec();

        if data.is_empty() {
            Err(anyhow!("empty blob"))
        } else {
            Ok(data)
        }
    }

    fn repo_path(&self, device_id: &str) -> String {
        format!("{}/{}", self.base_repo, device_id)
    }

    fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            req.basic_auth(u, Some(p))
        } else {
            req
        }
    }
}
