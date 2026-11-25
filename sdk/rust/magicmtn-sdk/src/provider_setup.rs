//! Setup runs explicitly during configuration, never during workload admission.
use crate::BoxFuture;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationOption {
    pub description: String,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSpec {
    pub options: BTreeMap<String, ConfigurationOption>,
}
impl SetupSpec {
    pub fn validate_options(&self, values: &BTreeMap<String, String>) -> Result<()> {
        ensure!(
            self.options.len() <= 32 && values.len() <= 32,
            "too many provider setup options"
        );
        for (key, option) in &self.options {
            ensure!(
                !key.is_empty()
                    && key.len() <= 64
                    && key.bytes().all(|b| b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b"_-".contains(&b))
                    && !option.description.is_empty()
                    && option.description.len() <= 512
                    && option
                        .default
                        .as_ref()
                        .is_none_or(|value| value.len() <= 1024 && !value.contains('\0')),
                "invalid provider setup declaration"
            );
        }
        for (key, value) in values {
            ensure!(
                self.options.contains_key(key),
                "unknown provider setup option"
            );
            ensure!(
                value.len() <= 1024 && !value.contains('\0'),
                "invalid provider setup option"
            );
        }
        Ok(())
    }
}

/// Nonsecret connection settings. The host stores credentials separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredConnection {
    pub account: String,
    pub configuration: BTreeMap<String, String>,
}
impl ConfiguredConnection {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.account.is_empty() && self.account.len() <= 512,
            "invalid provider account"
        );
        ensure!(
            self.configuration.len() <= 32,
            "too many provider connection settings"
        );
        for (key, value) in &self.configuration {
            ensure!(
                key.starts_with("MAGICMTN_")
                    && key.len() <= 96
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                    && value.len() <= 65536
                    && !value.contains('\0'),
                "invalid provider connection setting"
            );
        }
        Ok(())
    }
}

/// Implement on a provider with explicit credentials already attached.
pub trait ProviderSetup {
    /// Idempotently create or select account resources and return their identities.
    /// Existing configured identities must never be silently replaced.
    fn configure<'a>(
        &'a self,
        options: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, ConfiguredConnection>;
    /// Read-only verification. Missing resources remain missing.
    fn verify(&self) -> BoxFuture<'_, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_rejects_unknown_options_and_unsafe_settings() {
        let spec = SetupSpec {
            options: [(
                "app".into(),
                ConfigurationOption {
                    description: "App name".into(),
                    default: Some("magicmtn".into()),
                },
            )]
            .into(),
        };
        assert!(spec
            .validate_options(&[("app".into(), "my-app".into())].into())
            .is_ok());
        assert!(spec
            .validate_options(&[("unknown".into(), "value".into())].into())
            .is_err());
        let mut connection = ConfiguredConnection {
            account: "account-a".into(),
            configuration: [("PATH".into(), "/untrusted".into())].into(),
        };
        assert!(connection.validate().is_err());
        connection.configuration = [("MAGICMTN_FIXTURE_APP_ID".into(), "app-a".into())].into();
        assert!(connection.validate().is_ok());
        assert!(serde_json::from_str::<crate::plugin::CredentialValues>(
            r#"{"TOKEN":"first","TOKEN":"second"}"#
        )
        .is_err());
    }
}
