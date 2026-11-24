//! Persistent secret storage for trusted integration services, not a sandbox API.
//!
//! Ingress/brokers authorize callers and resolve `CredentialScope` from durable
//! connection records. Possession of a credential UUID is never authorization.
//! This module supplies storage isolation and authenticated encryption; it does
//! not implement integration grants, OAuth refresh, or external revocation.

use axum::async_trait;
use chrono::{DateTime, Utc};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::PgPool;
use std::{collections::BTreeMap, fmt, sync::Arc};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(feature = "credential-vault-aws")]
pub mod aws;

pub const MAX_SECRET_BYTES: usize = 65_536;
pub type Result<T> = std::result::Result<T, VaultError>;

/// Intentionally contains no database, SDK, request, or secret-bearing sources.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("invalid credential vault input")]
    InvalidInput,
    #[error("credential not found")]
    NotFound,
    #[error("credential version conflict")]
    Conflict,
    #[error("credential vault unavailable")]
    Unavailable,
    #[error("credential integrity check failed")]
    Integrity,
}

/// No serialization, implicit string conversion, or plaintext Debug output.
/// Zeroization is best effort; cryptographic libraries/SDKs may make copies.
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}

/// Organization-owned when `owner_user_id` is None; otherwise personally owned
/// within that organization. All fields are immutable and authenticated.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialScope {
    pub organization_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub connection_id: Uuid,
    /// Non-secret adapter-defined purpose, e.g. `github.installation`.
    pub purpose: String,
}

impl CredentialScope {
    fn validate(&self) -> Result<()> {
        let bytes = self.purpose.as_bytes();
        if self.organization_id.is_nil()
            || self.connection_id.is_nil()
            || self.owner_user_id.is_some_and(|id| id.is_nil())
            || bytes.is_empty()
            || bytes.len() > 64
            || !bytes[0].is_ascii_lowercase()
            || !bytes
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(c))
        {
            return Err(VaultError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialMetadata {
    pub id: Uuid,
    pub scope: CredentialScope,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct StoredCredential {
    pub metadata: CredentialMetadata,
    pub secret: SecretBytes,
}

/// Backend-independent contract. IDs survive backend migrations. `replace` and
/// `destroy` require compare-and-set versions; destroyed IDs cannot be reused.
/// Secrets are opaque bytes (typically a serialized adapter token bundle).
#[async_trait]
pub trait CredentialVault: Send + Sync {
    /// Exercise durable storage permissions and a managed-key round trip.
    async fn check_health(&self) -> Result<()>;
    async fn metadata(&self, scope: &CredentialScope, id: Uuid) -> Result<CredentialMetadata>;
    async fn create(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        secret: &SecretBytes,
    ) -> Result<CredentialMetadata>;
    async fn read(&self, scope: &CredentialScope, id: Uuid) -> Result<StoredCredential>;
    async fn replace(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        expected_version: i64,
        secret: &SecretBytes,
    ) -> Result<CredentialMetadata>;
    async fn destroy(&self, scope: &CredentialScope, id: Uuid, expected_version: i64)
        -> Result<()>;
}

/// Authenticated non-secret identity supplied to both KMS and AES-GCM.
/// Never put tokens, emails, or other secret values in this context: AWS logs it.
#[derive(Debug, Clone)]
pub struct EncryptionContext(BTreeMap<String, String>);

impl EncryptionContext {
    fn new(scope: &CredentialScope, id: Uuid, version: i64) -> Self {
        Self(BTreeMap::from([
            ("application".into(), "magicmtn-credential-vault-v1".into()),
            ("organization_id".into(), scope.organization_id.to_string()),
            (
                "owner_user_id".into(),
                scope
                    .owner_user_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "organization".into()),
            ),
            ("connection_id".into(), scope.connection_id.to_string()),
            ("purpose".into(), scope.purpose.clone()),
            ("credential_id".into(), id.to_string()),
            ("version".into(), version.to_string()),
        ]))
    }

    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    fn aad(&self) -> Vec<u8> {
        // BTreeMap and string-only values provide a canonical encoding.
        serde_json::to_vec(&self.0).expect("string map is serializable")
    }
}

pub struct DataKey {
    pub plaintext: SecretBytes,
    pub encrypted: Vec<u8>,
    /// Persist the resolved key ID/ARN, not a mutable alias.
    pub key_id: String,
}

/// Small seam for managed key services and credential-free test fixtures.
/// Production has no plaintext/local-key fallback.
#[async_trait]
pub trait DataKeyProvider: Send + Sync {
    async fn generate(&self, context: &EncryptionContext) -> Result<DataKey>;
    async fn decrypt(
        &self,
        context: &EncryptionContext,
        key_id: &str,
        encrypted: &[u8],
    ) -> Result<SecretBytes>;
}

#[derive(Clone)]
pub struct PostgresKmsVault {
    pool: PgPool,
    keys: Arc<dyn DataKeyProvider>,
}

#[derive(sqlx::FromRow)]
struct Entry {
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    key_id: String,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

const COLUMNS: &str = "version,created_at,updated_at,key_id,encrypted_data_key,nonce,ciphertext";
const SCOPE_FILTER: &str = "organization_id=$1 AND owner_user_id IS NOT DISTINCT FROM $2 AND connection_id=$3 AND purpose=$4 AND id=$5 AND destroyed_at IS NULL AND backend='postgres-kms-v1'";

impl Entry {
    fn metadata(&self, scope: &CredentialScope, id: Uuid) -> CredentialMetadata {
        CredentialMetadata {
            id,
            scope: scope.clone(),
            version: self.version,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl PostgresKmsVault {
    /// Use a trusted, non-BYPASSRLS pool with the repository's organization RLS
    /// configuration. Workers may use their trusted worker pool; every query
    /// additionally matches the complete explicit scope.
    pub fn new(pool: PgPool, keys: Arc<dyn DataKeyProvider>) -> Self {
        Self { pool, keys }
    }

    async fn entry(&self, scope: &CredentialScope, id: Uuid) -> Result<Entry> {
        scope.validate()?;
        if id.is_nil() {
            return Err(VaultError::InvalidInput);
        }
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM credential_vault_entries WHERE {SCOPE_FILTER}"
        ))
        .bind(scope.organization_id)
        .bind(scope.owner_user_id)
        .bind(scope.connection_id)
        .bind(&scope.purpose)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| VaultError::Unavailable)?
        .ok_or(VaultError::NotFound)
    }

    async fn encrypt(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        version: i64,
        secret: &SecretBytes,
    ) -> Result<Envelope> {
        if secret.expose_secret().is_empty() || secret.expose_secret().len() > MAX_SECRET_BYTES {
            return Err(VaultError::InvalidInput);
        }
        let context = EncryptionContext::new(scope, id, version);
        let key = self.keys.generate(&context).await?;
        if key.key_id.is_empty() || key.encrypted.is_empty() {
            return Err(VaultError::Integrity);
        }
        let mut nonce = [0; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| VaultError::Unavailable)?;
        let mut ciphertext = Zeroizing::new(secret.expose_secret().to_vec());
        cipher(&key.plaintext)?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(context.aad()),
                &mut *ciphertext,
            )
            .map_err(|_| VaultError::Integrity)?;
        Ok(Envelope {
            key_id: key.key_id,
            encrypted_data_key: key.encrypted,
            nonce: nonce.to_vec(),
            ciphertext: ciphertext.to_vec(),
        })
    }
}

struct Envelope {
    key_id: String,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn cipher(key: &SecretBytes) -> Result<aead::LessSafeKey> {
    Ok(aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, key.expose_secret())
            .map_err(|_| VaultError::Integrity)?,
    ))
}

fn next_version(version: i64) -> Result<i64> {
    if version < 1 {
        return Err(VaultError::InvalidInput);
    }
    version.checked_add(1).ok_or(VaultError::InvalidInput)
}

#[async_trait]
impl CredentialVault for PostgresKmsVault {
    async fn check_health(&self) -> Result<()> {
        // No fixture credentials are persisted by readiness checks.
        sqlx::query("SELECT id FROM credential_vault_entries WHERE false")
            .execute(&self.pool)
            .await
            .map_err(|_| VaultError::Unavailable)?;
        let scope = CredentialScope {
            organization_id: Uuid::new_v4(),
            owner_user_id: None,
            connection_id: Uuid::new_v4(),
            purpose: "health".into(),
        };
        let context = EncryptionContext::new(&scope, Uuid::new_v4(), 1);
        let key = self.keys.generate(&context).await?;
        let decoded = self
            .keys
            .decrypt(&context, &key.key_id, &key.encrypted)
            .await?;
        if key.plaintext.expose_secret().len() != 32
            || decoded.expose_secret() != key.plaintext.expose_secret()
        {
            return Err(VaultError::Integrity);
        }
        Ok(())
    }

    async fn metadata(&self, scope: &CredentialScope, id: Uuid) -> Result<CredentialMetadata> {
        Ok(self.entry(scope, id).await?.metadata(scope, id))
    }

    async fn create(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        secret: &SecretBytes,
    ) -> Result<CredentialMetadata> {
        scope.validate()?;
        if id.is_nil() {
            return Err(VaultError::InvalidInput);
        }
        let envelope = self.encrypt(scope, id, 1, secret).await?;
        let entry: Entry = sqlx::query_as(&format!("INSERT INTO credential_vault_entries(id,organization_id,owner_user_id,connection_id,purpose,version,key_id,encrypted_data_key,nonce,ciphertext) VALUES($1,$2,$3,$4,$5,1,$6,$7,$8,$9) RETURNING {COLUMNS}"))
            .bind(id).bind(scope.organization_id).bind(scope.owner_user_id).bind(scope.connection_id).bind(&scope.purpose)
            .bind(envelope.key_id).bind(envelope.encrypted_data_key).bind(envelope.nonce).bind(envelope.ciphertext)
            .fetch_one(&self.pool).await.map_err(|e| {
                if e.as_database_error().is_some_and(|e| e.is_unique_violation()) { VaultError::Conflict } else { VaultError::Unavailable }
            })?;
        Ok(entry.metadata(scope, id))
    }

    async fn read(&self, scope: &CredentialScope, id: Uuid) -> Result<StoredCredential> {
        let entry = self.entry(scope, id).await?;
        let context = EncryptionContext::new(scope, id, entry.version);
        let key = self
            .keys
            .decrypt(&context, &entry.key_id, &entry.encrypted_data_key)
            .await?;
        let nonce: [u8; 12] = entry
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Integrity)?;
        let mut plaintext = Zeroizing::new(entry.ciphertext.clone());
        let decrypted = cipher(&key)?
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(context.aad()),
                &mut plaintext,
            )
            .map_err(|_| VaultError::Integrity)?;
        let secret = SecretBytes::new(decrypted.to_vec());
        Ok(StoredCredential {
            metadata: entry.metadata(scope, id),
            secret,
        })
    }

    async fn replace(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        expected_version: i64,
        secret: &SecretBytes,
    ) -> Result<CredentialMetadata> {
        let version = next_version(expected_version)?;
        let current = self.entry(scope, id).await?;
        if current.version != expected_version {
            return Err(VaultError::Conflict);
        }
        let envelope = self.encrypt(scope, id, version, secret).await?;
        let entry: Option<Entry> = sqlx::query_as(&format!("UPDATE credential_vault_entries SET version=$6,key_id=$7,encrypted_data_key=$8,nonce=$9,ciphertext=$10,updated_at=clock_timestamp() WHERE {SCOPE_FILTER} AND version=$11 RETURNING {COLUMNS}"))
            .bind(scope.organization_id).bind(scope.owner_user_id).bind(scope.connection_id).bind(&scope.purpose).bind(id)
            .bind(version).bind(envelope.key_id).bind(envelope.encrypted_data_key).bind(envelope.nonce).bind(envelope.ciphertext).bind(expected_version)
            .fetch_optional(&self.pool).await.map_err(|_| VaultError::Unavailable)?;
        Ok(entry.ok_or(VaultError::Conflict)?.metadata(scope, id))
    }

    async fn destroy(
        &self,
        scope: &CredentialScope,
        id: Uuid,
        expected_version: i64,
    ) -> Result<()> {
        let version = next_version(expected_version)?;
        let current = self.entry(scope, id).await?;
        if current.version != expected_version {
            return Err(VaultError::Conflict);
        }
        let changed = sqlx::query(&format!("UPDATE credential_vault_entries SET version=$6,key_id=NULL,encrypted_data_key=NULL,nonce=NULL,ciphertext=NULL,updated_at=clock_timestamp(),destroyed_at=clock_timestamp() WHERE {SCOPE_FILTER} AND version=$7"))
            .bind(scope.organization_id).bind(scope.owner_user_id).bind(scope.connection_id).bind(&scope.purpose).bind(id).bind(version).bind(expected_version)
            .execute(&self.pool).await.map_err(|_| VaultError::Unavailable)?.rows_affected();
        if changed == 0 {
            return Err(VaultError::Conflict);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_and_versions_are_bounded() {
        let secret = SecretBytes::new(b"do-not-log-this".to_vec());
        assert_eq!(format!("{secret:?}"), "SecretBytes([REDACTED])");
        assert_eq!(next_version(0), Err(VaultError::InvalidInput));
        assert_eq!(next_version(i64::MAX), Err(VaultError::InvalidInput));
        assert_eq!(next_version(1), Ok(2));
    }

    #[test]
    fn context_binds_all_identity_fields_and_version() {
        let mut scope = CredentialScope {
            organization_id: Uuid::new_v4(),
            owner_user_id: None,
            connection_id: Uuid::new_v4(),
            purpose: "github.oauth".into(),
        };
        let id = Uuid::new_v4();
        let original = EncryptionContext::new(&scope, id, 1).aad();
        assert_ne!(original, EncryptionContext::new(&scope, id, 2).aad());
        assert_ne!(
            original,
            EncryptionContext::new(&scope, Uuid::new_v4(), 1).aad()
        );
        let other_scopes = [
            CredentialScope {
                organization_id: Uuid::new_v4(),
                ..scope.clone()
            },
            CredentialScope {
                owner_user_id: Some(Uuid::new_v4()),
                ..scope.clone()
            },
            CredentialScope {
                connection_id: Uuid::new_v4(),
                ..scope.clone()
            },
            CredentialScope {
                purpose: "github.installation".into(),
                ..scope.clone()
            },
        ];
        for other in other_scopes {
            assert_ne!(original, EncryptionContext::new(&other, id, 1).aad());
        }
        scope.purpose = "secret token!".into();
        assert_eq!(scope.validate(), Err(VaultError::InvalidInput));
    }
}
