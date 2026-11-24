//! AWS Secrets Manager supplies externally managed, versioned provider bundles.
use super::{parse_bundle, CredentialBundle, CredentialSource};
use anyhow::{ensure, Context, Result};
use aws_sdk_secretsmanager::Client;
use axum::async_trait;

pub struct SecretsManagerSource {
    client: Client,
    arn: String,
}

impl SecretsManagerSource {
    pub fn new(client: Client, arn: String) -> Result<Self> {
        let parts: Vec<_> = arn.splitn(6, ':').collect();
        ensure!(
            parts.len() == 6
                && parts[0] == "arn"
                && parts[1].starts_with("aws")
                && parts[2] == "secretsmanager"
                && !parts[3].is_empty()
                && parts[4].len() == 12
                && parts[4].bytes().all(|byte| byte.is_ascii_digit())
                && parts[5]
                    .strip_prefix("secret:")
                    .is_some_and(|name| !name.is_empty()),
            "expected a Secrets Manager secret ARN"
        );
        Ok(Self { client, arn })
    }
}

#[async_trait]
impl CredentialSource for SecretsManagerSource {
    async fn load(&self) -> Result<CredentialBundle> {
        let mut response = self
            .client
            .get_secret_value()
            .secret_id(&self.arn)
            .version_stage("AWSCURRENT")
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Secrets Manager credential unavailable"))?;
        ensure!(
            response.arn() == Some(self.arn.as_str()),
            "Secrets Manager returned an unexpected secret"
        );
        let version = response
            .version_id
            .take()
            .context("Secrets Manager response has no version")?;
        let plaintext = zeroize::Zeroizing::new(
            response
                .secret_string
                .take()
                .context("Secrets Manager credential must be a JSON SecretString")?,
        );
        Ok(CredentialBundle {
            version,
            values: parse_bundle(plaintext.as_bytes())?,
        })
    }
}
