use super::{Client, Pin, ProcessEnvironment, Registry};
use anyhow::{ensure, Context, Result};
use magicmtn_sdk::plugin::{CredentialValues, Value, CREDENTIAL_PROTOCOL_VERSION};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The caller derives this from an authorized, durable connection binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    pub organization: String,
    pub connection: String,
    pub provider: String,
    pub reference: String,
}

pub struct PreparedCredential {
    pub version: String,
    pub account: String,
    pub values: Arc<CredentialValues>,
    pub expires_at: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Credentials(Mutex<HashMap<CredentialRef, Slot>>);

#[derive(Default)]
struct Slot {
    revoked: bool,
    ticket: Option<Arc<()>>,
    account: Option<String>,
    active: Option<Arc<Active>>,
    expires_at: Option<Instant>,
}

#[derive(Hash, PartialEq, Eq)]
struct ClientKey {
    pin: Pin,
    scope: String,
}

type ClientSlot = Arc<tokio::sync::Mutex<Option<Arc<Client>>>>;

struct Active {
    credential: PreparedCredential,
    clients: Mutex<HashMap<ClientKey, ClientSlot>>,
}

/// A single publication attempt. New updates and revocations fence old attempts.
pub struct CredentialUpdate {
    key: CredentialRef,
    ticket: Arc<()>,
}

/// A snapshot captured at admission. Rotations route new admissions to the new
/// version while an admitted operation retains its original credential.
pub struct CredentialAdmission {
    pin: Pin,
    active: Arc<Active>,
}

impl Registry {
    pub fn begin_credential_update(&self, key: CredentialRef) -> Result<CredentialUpdate> {
        let ticket = Arc::new(());
        let mut slots = self.credentials.0.lock().unwrap();
        let slot = slots.entry(key.clone()).or_default();
        ensure!(!slot.revoked, "provider credential revoked");
        slot.ticket = Some(ticket.clone());
        Ok(CredentialUpdate { key, ticket })
    }

    pub fn publish_credential(
        &self,
        update: CredentialUpdate,
        candidate: PreparedCredential,
    ) -> Result<()> {
        ensure!(
            !candidate.account.is_empty() && !candidate.version.is_empty(),
            "invalid prepared provider credential"
        );
        let mut slots = self.credentials.0.lock().unwrap();
        let slot = slots
            .get_mut(&update.key)
            .context("credential update superseded")?;
        ensure!(
            slot.ticket
                .as_ref()
                .is_some_and(|ticket| Arc::ptr_eq(ticket, &update.ticket)),
            "credential update superseded"
        );
        ensure!(
            slot.account
                .as_ref()
                .is_none_or(|account| account == &candidate.account),
            "provider account differs from the connection"
        );
        slot.ticket = None;
        slot.account = Some(candidate.account.clone());
        slot.expires_at = candidate.expires_at;
        let old = if slot
            .active
            .as_ref()
            .is_none_or(|active| active.credential.version != candidate.version)
        {
            slot.active.replace(Arc::new(Active {
                credential: candidate,
                clients: Mutex::default(),
            }))
        } else {
            None
        };
        drop(slots);
        drop(old);
        Ok(())
    }

    pub fn renew_credential(
        &self,
        update: &CredentialUpdate,
        version: &str,
        account: &str,
        expires_at: Option<Instant>,
    ) -> Result<bool> {
        let mut slots = self.credentials.0.lock().unwrap();
        let slot = slots
            .get_mut(&update.key)
            .context("credential update superseded")?;
        ensure!(
            slot.ticket
                .as_ref()
                .is_some_and(|ticket| Arc::ptr_eq(ticket, &update.ticket)),
            "credential update superseded"
        );
        ensure!(
            slot.account
                .as_deref()
                .is_none_or(|current| current == account),
            "provider account differs from the connection"
        );
        if slot
            .active
            .as_ref()
            .is_some_and(|active| active.credential.version == version)
        {
            slot.expires_at = expires_at;
            slot.ticket = None;
            return Ok(true);
        }
        Ok(false)
    }

    /// Prevent new admissions and refreshes until the operator re-enables this binding.
    pub fn revoke_credential(&self, key: &CredentialRef) {
        let old = {
            let mut slots = self.credentials.0.lock().unwrap();
            let slot = slots.entry(key.clone()).or_default();
            slot.revoked = true;
            slot.ticket = None;
            slot.active.take()
        };
        drop(old);
    }

    pub fn enable_credential(&self, key: &CredentialRef) {
        if let Some(slot) = self.credentials.0.lock().unwrap().get_mut(key) {
            slot.revoked = false;
        }
    }

    /// Check an account during preparation. Dispatch startup skips this query.
    pub async fn verify_credential(
        &self,
        pin: &Pin,
        environment: &ProcessEnvironment,
        values: &CredentialValues,
    ) -> Result<String> {
        let (installed, executable) = self.executable(pin)?;
        let spec = installed
            .manifest
            .credential
            .context("provider has no credential declaration")?;
        spec.check_values(values)?;
        ensure!(
            spec.fields
                .iter()
                .all(|field| !environment.values.contains_key(field)),
            "provider credential conflicts with configuration"
        );
        let configured = self.configured_environment(pin, Some(environment))?;
        let (mut connection, _scratch) = Client::spawn(
            &executable,
            CREDENTIAL_PROTOCOL_VERSION,
            Some(configured.as_ref().unwrap_or(environment)),
        )?;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            connection.startup(&pin.name, Some(values), true),
        )
        .await;
        match result {
            Ok(Ok(Value::VerifiedAccount(account)))
                if !account.is_empty() && account.len() <= 512 =>
            {
                Ok(account)
            }
            _ => anyhow::bail!("provider credential verification failed"),
        }
    }

    /// Explicit setup may provision resources; normal startup never calls it.
    pub async fn configure_credential(
        &self,
        pin: &Pin,
        environment: &ProcessEnvironment,
        values: &CredentialValues,
        options: &std::collections::BTreeMap<String, String>,
    ) -> Result<magicmtn_sdk::provider_setup::ConfiguredConnection> {
        use magicmtn_sdk::{plugin::Operation, provider_setup::ConfiguredConnection};
        let (installed, executable) = self.executable(pin)?;
        let Some(setup) = &installed.manifest.capabilities.setup else {
            ensure!(options.is_empty(), "provider has no setup options");
            return Ok(ConfiguredConnection {
                account: self.verify_credential(pin, environment, values).await?,
                configuration: Default::default(),
            });
        };
        setup.validate_options(options)?;
        let spec = installed
            .manifest
            .credential
            .as_ref()
            .context("provider has no credentials")?;
        spec.check_values(values)?;
        ensure!(
            spec.fields
                .iter()
                .all(|field| !environment.values.contains_key(field)),
            "provider credential conflicts with configuration"
        );
        let configured = self.configured_environment(pin, Some(environment))?;
        let client = Client::start_with_credential(
            &executable,
            &pin.name,
            installed.package.protocol_version,
            Some(configured.as_ref().unwrap_or(environment)),
            Some(values),
        )
        .await?;
        ensure!(
            client.manifest == installed.manifest,
            "provider manifest changed"
        );
        let result = client
            .call(
                Operation::ProviderConfigure {
                    options: options.clone(),
                },
                Duration::from_secs(60),
            )
            .await;
        client.shutdown().await;
        match result {
            Ok(Value::ConfiguredConnection(connection)) => {
                connection.validate()?;
                let prefix = format!("MAGICMTN_{}_", pin.name.to_uppercase().replace('-', "_"));
                ensure!(
                    connection
                        .configuration
                        .keys()
                        .all(|key| key.starts_with(&prefix)
                            && !key.ends_with("_PYTHON")
                            && !key.ends_with("_KUBECTL")
                            && !installed
                                .package
                                .runtime
                                .as_ref()
                                .is_some_and(|runtime| runtime.environment.contains_key(key))),
                    "provider returned a setting outside its connection scope"
                );
                ensure!(
                    spec.fields
                        .iter()
                        .all(|field| !connection.configuration.contains_key(field)),
                    "provider returned credential fields in configuration"
                );
                Ok(connection)
            }
            _ => anyhow::bail!("provider setup failed"),
        }
    }

    /// Explicit read-only check of the currently prepared connection.
    pub async fn verify_connection(
        &self,
        key: &CredentialRef,
        pin: &Pin,
        environment: &ProcessEnvironment,
    ) -> Result<()> {
        let admission = self.admit_credential(key, pin)?;
        let account = self
            .verify_credential(pin, environment, &admission.active.credential.values)
            .await?;
        ensure!(
            account == admission.active.credential.account,
            "provider account differs from the connection"
        );
        Ok(())
    }

    pub fn admit_credential(&self, key: &CredentialRef, pin: &Pin) -> Result<CredentialAdmission> {
        ensure!(
            key.provider == pin.name,
            "provider credential identity mismatch"
        );
        let active = {
            let slots = self.credentials.0.lock().unwrap();
            let slot = slots.get(key).context("provider credential unavailable")?;
            ensure!(
                slot.expires_at
                    .is_none_or(|deadline| Instant::now() < deadline),
                "provider credential expired"
            );
            slot.active
                .clone()
                .context("provider credential unavailable")?
        };
        Ok(CredentialAdmission {
            active,
            pin: pin.clone(),
        })
    }

    /// Resolve a process using an admitted credential. Scope includes the
    /// immutable binding generation within the resource's process boundary.
    pub async fn prepared_client(
        &self,
        admission: CredentialAdmission,
        scope: &str,
        configuration: &std::collections::BTreeMap<String, String>,
    ) -> Result<Arc<Client>> {
        let CredentialAdmission { pin, active } = admission;
        let pin = &pin;
        let cache_key = ClientKey {
            pin: pin.clone(),
            scope: scope.into(),
        };
        let slot = active
            .clients
            .lock()
            .unwrap()
            .entry(cache_key)
            .or_default()
            .clone();
        let mut client = slot.lock().await;
        if let Some(current) = client.as_ref() {
            if current
                .connection
                .try_lock()
                .map_or(true, |state| state.is_some())
            {
                return Ok(current.clone());
            }
        }
        let (installed, executable) = self.executable(pin)?;
        let spec = installed
            .manifest
            .credential
            .as_ref()
            .context("provider has no credential declaration")?;
        spec.check_values(&active.credential.values)?;
        ensure!(
            spec.fields
                .iter()
                .all(|field| !configuration.contains_key(field)),
            "provider credential conflicts with configuration"
        );
        let environment = ProcessEnvironment {
            values: configuration.clone(),
        };
        let configured = self.configured_environment(pin, Some(&environment))?;
        let started = Arc::new(
            Client::start_with_credential(
                &executable,
                &pin.name,
                installed.package.protocol_version,
                Some(configured.as_ref().unwrap_or(&environment)),
                Some(&active.credential.values),
            )
            .await?,
        );
        ensure!(
            started.manifest == installed.manifest,
            "provider manifest changed"
        );
        *client = Some(started.clone());
        Ok(started)
    }
}

impl Credentials {
    pub(super) fn release(&self, scope: &str) {
        let prefix = format!("{scope}:");
        let active: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .values()
            .filter_map(|slot| slot.active.clone())
            .collect();
        for credential in active {
            credential
                .clients
                .lock()
                .unwrap()
                .retain(|key, _| key.scope != scope && !key.scope.starts_with(&prefix));
        }
    }

    pub(super) fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_and_other_connection_credentials_cannot_be_admitted() {
        let directory = tempfile::tempdir().unwrap();
        let registry = Registry::new(directory.path().to_owned()).unwrap();
        let key = CredentialRef {
            organization: "workspace-a".into(),
            connection: "connection-a".into(),
            provider: "modal".into(),
            reference: "provider-modal".into(),
        };
        let fixture = || PreparedCredential {
            version: "v1".into(),
            account: "account-a".into(),
            values: Arc::new(CredentialValues(Default::default())),
            expires_at: None,
        };
        registry
            .publish_credential(
                registry.begin_credential_update(key.clone()).unwrap(),
                fixture(),
            )
            .unwrap();
        let stale = registry.begin_credential_update(key.clone()).unwrap();
        registry.revoke_credential(&key);
        assert!(registry.begin_credential_update(key.clone()).is_err());
        assert!(registry.publish_credential(stale, fixture()).is_err());

        let pin = Pin {
            name: "modal".into(),
            version: "1.0.0".into(),
            sha256: "fixture".into(),
        };
        assert_eq!(
            registry
                .admit_credential(&key, &pin)
                .err()
                .unwrap()
                .to_string(),
            "provider credential unavailable"
        );
        registry.enable_credential(&key);
        registry
            .publish_credential(
                registry.begin_credential_update(key.clone()).unwrap(),
                fixture(),
            )
            .unwrap();
        for other in [
            CredentialRef {
                organization: "workspace-b".into(),
                ..key.clone()
            },
            CredentialRef {
                connection: "connection-b".into(),
                ..key.clone()
            },
            CredentialRef {
                reference: "provider-modal-other".into(),
                ..key.clone()
            },
        ] {
            assert_eq!(
                registry
                    .admit_credential(&other, &pin)
                    .err()
                    .unwrap()
                    .to_string(),
                "provider credential unavailable"
            );
        }
    }
}
