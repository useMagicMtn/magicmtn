//! Credential source reads are confined to administrative and background preparation.
use crate::{
    credential_vault::{CredentialScope, CredentialVault},
    provider_connections::Binding,
};
use anyhow::{ensure, Context, Result};
use axum::async_trait;
use magicmtn_provider_framework::host::{
    CredentialRef, PreparedCredential, ProcessEnvironment, Registry,
};
use magicmtn_sdk::plugin::CredentialValues;
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[cfg(feature = "provider-credentials-aws")]
pub mod aws;
#[cfg(feature = "provider-credentials-vault")]
pub mod vault_kv;

pub const MAX_BUNDLE_BYTES: usize = 65_536;

/// Persist the upstream account identity with a managed reference. Changing
/// the account creates a different binding for new resources.
pub fn managed_reference(profile: &str, account: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    ensure!(
        profile.starts_with("managed:")
            && !profile.contains('#')
            && !account.is_empty()
            && account.len() <= 512,
        "invalid managed provider account"
    );
    Ok(format!(
        "{profile}#{:x}",
        Sha256::digest(account.as_bytes())
    ))
}

pub fn reference(binding: &Binding) -> Result<CredentialRef> {
    ensure!(
        binding.runner_id.is_none(),
        "provider credential must be prepared on the execution host"
    );
    let reference = binding
        .credential_ref
        .clone()
        .context("provider connection has no credential reference")?;
    Ok(CredentialRef {
        organization: binding.organization_id.to_string(),
        connection: binding.connection_id.to_string(),
        provider: binding.pin.name.clone(),
        reference,
    })
}

pub fn parse_bundle(bytes: &[u8]) -> Result<CredentialValues> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BUNDLE_BYTES,
        "provider credential exceeds the size limit or is empty"
    );
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let credentials = serde::de::Deserializer::deserialize_map(&mut deserializer, BundleVisitor)
        .map_err(|_| anyhow::anyhow!("expected a JSON object with unique string fields"))?;
    deserializer
        .end()
        .map_err(|_| anyhow::anyhow!("expected a single JSON credential object"))?;
    Ok(credentials)
}

struct BundleVisitor;
impl<'de> serde::de::Visitor<'de> for BundleVisitor {
    type Value = CredentialValues;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON credential object")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = CredentialValues(Default::default());
        while let Some(key) = map.next_key::<String>()? {
            if values.0.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate credential field"));
            }
            let value = map.next_value::<String>()?;
            values.0.insert(key, value);
        }
        Ok(values)
    }
}

pub struct CredentialBundle {
    pub version: String,
    pub values: CredentialValues,
}

/// Implementations are constructed with a trusted store locator and identity.
/// Provider operations never receive a source or a store locator.
#[async_trait]
pub trait CredentialSource: Send + Sync {
    async fn load(&self) -> Result<CredentialBundle>;
}

pub struct VaultSource<'a> {
    pub vault: &'a dyn CredentialVault,
    pub scope: CredentialScope,
    pub id: Uuid,
}

#[async_trait]
impl CredentialSource for VaultSource<'_> {
    async fn load(&self) -> Result<CredentialBundle> {
        let entry = self.vault.read(&self.scope, self.id).await?;
        Ok(CredentialBundle {
            version: entry.metadata.version.to_string(),
            values: parse_bundle(entry.secret.expose_secret())?,
        })
    }
}

/// Called from startup or a background task using a trusted binding and the
/// account identity pinned by the operator. Failed reads retain the prior
/// credential until its previously established deadline.
pub async fn refresh(
    registry: &Registry,
    binding: &Binding,
    source: &dyn CredentialSource,
    expected_account: &str,
    max_staleness: Duration,
) -> Result<()> {
    refresh_bindings(
        registry,
        std::slice::from_ref(binding),
        source,
        expected_account,
        max_staleness,
    )
    .await
}

/// Fan out one read for an operator-managed account to its authorized bindings
/// on this replica. Each binding retains independent authorization and fencing.
pub async fn refresh_bindings(
    registry: &Registry,
    bindings: &[Binding],
    source: &dyn CredentialSource,
    expected_account: &str,
    max_staleness: Duration,
) -> Result<()> {
    let Some(first) = bindings.first() else {
        return Ok(());
    };
    if let Some(reference) = first
        .credential_ref
        .as_deref()
        .filter(|value| value.starts_with("managed:"))
    {
        let (profile, _) = reference
            .split_once('#')
            .context("managed credential account is not pinned")?;
        ensure!(
            managed_reference(profile, expected_account)? == reference,
            "provider account differs from the connection"
        );
    }
    let mut keys = HashSet::new();
    for binding in bindings {
        ensure!(
            binding.pin == first.pin
                && binding.configuration == first.configuration
                && binding.credential_ref == first.credential_ref,
            "shared provider bindings must use one release, configuration, and account reference"
        );
        keys.insert(reference(binding)?);
    }
    let mut updates = Vec::new();
    let mut error = None;
    for key in keys {
        match registry.begin_credential_update(key) {
            Ok(update) => updates.push(update),
            Err(cause) => {
                error = Some(cause);
            }
        }
    }
    if updates.is_empty() {
        return error.map_or(Ok(()), Err);
    }
    let bundle = source.load().await?;
    let expires_at = Some(
        Instant::now()
            .checked_add(max_staleness)
            .context("invalid credential lifetime")?,
    );
    let mut pending = Vec::new();
    for update in updates {
        match registry.renew_credential(&update, &bundle.version, expected_account, expires_at) {
            Ok(true) => {}
            Ok(false) => pending.push(update),
            Err(cause) => {
                error = Some(cause);
            }
        }
    }
    if !pending.is_empty() {
        let account = registry
            .verify_credential(
                &first.pin,
                &ProcessEnvironment {
                    values: first.configuration.clone(),
                },
                &bundle.values,
            )
            .await?;
        ensure!(
            account == expected_account,
            "provider account differs from the connection"
        );
        let values = Arc::new(bundle.values);
        for update in pending {
            let prepared = PreparedCredential {
                version: bundle.version.clone(),
                account: account.clone(),
                values: values.clone(),
                expires_at,
            };
            if let Err(cause) = registry.publish_credential(update, prepared) {
                error = Some(cause);
            }
        }
    }
    error.map_or(Ok(()), Err)
}
