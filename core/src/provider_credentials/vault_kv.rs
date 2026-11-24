//! Read HashiCorp Vault and OpenBao KV v2 with an operator-provisioned token file.
use super::{CredentialBundle, CredentialSource, MAX_BUNDLE_BYTES};
use anyhow::{ensure, Context, Result};
use axum::async_trait;
use magicmtn_sdk::plugin::CredentialValues;
use reqwest::{header::HeaderValue, Client, ClientBuilder, Url};
use serde::Deserialize;
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

pub struct VaultKvSource {
    client: Client,
    url: Url,
    token_file: PathBuf,
}

impl VaultKvSource {
    pub fn new(
        builder: ClientBuilder,
        mut endpoint: Url,
        mount: &str,
        key: &str,
        token_file: PathBuf,
    ) -> Result<Self> {
        ensure!(
            endpoint.scheme() == "https"
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "expected a Vault HTTPS endpoint"
        );
        let segments = |value: &str| {
            value
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
        };
        ensure!(
            segments(mount) && segments(key),
            "invalid Vault KV mount or key"
        );
        endpoint
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid Vault endpoint"))?
            .pop_if_empty()
            .push("v1")
            .extend(mount.split('/'))
            .push("data")
            .extend(key.split('/'));
        Ok(Self {
            client: builder
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            url: endpoint,
            token_file,
        })
    }
}

#[derive(Deserialize)]
struct ReadResponse {
    data: KvVersion,
}

#[derive(Deserialize)]
struct KvVersion {
    data: CredentialValues,
    metadata: Metadata,
}

#[derive(Deserialize)]
struct Metadata {
    version: u64,
    created_time: String,
}

#[async_trait]
impl CredentialSource for VaultKvSource {
    async fn load(&self) -> Result<CredentialBundle> {
        let mut token = Zeroizing::new(String::new());
        tokio::fs::File::open(&self.token_file)
            .await
            .context("opening Vault identity file")?
            .take(8193)
            .read_to_string(&mut token)
            .await
            .context("reading Vault identity file")?;
        ensure!(
            !token.trim().is_empty() && token.len() <= 8192,
            "invalid Vault identity"
        );
        let mut header = HeaderValue::from_str(token.trim())
            .map_err(|_| anyhow::anyhow!("invalid Vault identity"))?;
        header.set_sensitive(true);
        let mut response = self
            .client
            .get(self.url.clone())
            .header("X-Vault-Token", header)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Vault credential unavailable"))?;
        ensure!(
            response.status().is_success(),
            "Vault credential unavailable"
        );
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Vault credential unavailable"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= MAX_BUNDLE_BYTES + 4096,
                "Vault response exceeds the size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let response: ReadResponse = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid Vault KV response"))?;
        let metadata = response.data.metadata;
        ensure!(
            metadata.version > 0 && !metadata.created_time.is_empty(),
            "invalid Vault version"
        );
        Ok(CredentialBundle {
            version: format!("{}:{}", metadata.version, metadata.created_time),
            values: response.data.data,
        })
    }
}
