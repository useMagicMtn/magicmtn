//! Persistence boundary for the shared sandbox service. PostgreSQL retains the
//! host's scoped pool; SQLite represents one local installation.
use crate::provider_connections::Binding;
use crate::sandboxes::{ApiError, Create, SandboxPolicy};
use anyhow::{ensure, Context, Result};
use magicmtn_sdk::ProviderSandbox;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[derive(Debug, thiserror::Error)]
#[error("select an explicit provider connection")]
pub struct ConnectionSelectionRequired;

#[derive(Clone)]
pub enum Store {
    Postgres(PgPool),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::Database),
}
impl From<PgPool> for Store {
    fn from(pool: PgPool) -> Self {
        Self::Postgres(pool)
    }
}
impl Store {
    pub async fn records(&self, prefix: Option<&str>) -> Result<Vec<Value>> {
        match self {
            Self::Postgres(pool) => Ok(match prefix {
                Some(prefix) => {
                    sqlx::query_scalar(
                        "SELECT to_jsonb(s) FROM sandboxes s WHERE id::text LIKE $1 LIMIT 2",
                    )
                    .bind(format!("{}%", prefix.to_ascii_lowercase()))
                    .fetch_all(pool)
                    .await?
                }
                None => {
                    sqlx::query_scalar(
                        "SELECT to_jsonb(s) FROM sandboxes s ORDER BY created_at DESC,id",
                    )
                    .fetch_all(pool)
                    .await?
                }
            }),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.records(prefix).await,
        }
    }
    pub async fn get(&self, id: Uuid) -> Result<Option<Value>> {
        match self {
            Self::Postgres(pool) => Ok(sqlx::query_scalar(
                "SELECT to_jsonb(b) FROM sandboxes b WHERE id=$1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.get(id).await,
        }
    }
    pub async fn binding(
        &self,
        id: Option<Uuid>,
        reference: Option<&str>,
        provider: &str,
    ) -> Result<Binding> {
        match self {
            Self::Postgres(pool) => {
                if let Some(id) = id {
                    return crate::provider_connections::by_id(pool, id).await;
                }
                if let Some(reference) = reference {
                    return crate::provider_connections::by_ref(
                        pool,
                        crate::workspace::current(pool).await?,
                        reference,
                        provider,
                    )
                    .await;
                }
                let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM provider_connections WHERE provider=$1 AND status='enabled' ORDER BY id LIMIT 2").bind(provider).fetch_all(pool).await?;
                ensure!(ids.len() == 1, ConnectionSelectionRequired);
                crate::provider_connections::by_id(pool, ids[0]).await
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.binding(id, reference, provider).await,
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Reserve a resource with its full placement and lifecycle policy"
    )]
    pub async fn reserve(
        &self,
        id: Uuid,
        body: &Create,
        metadata: &Value,
        snapshot_key: &str,
        user: Option<Uuid>,
        ttl: u32,
        policy: &dyn SandboxPolicy,
    ) -> Result<(), ApiError> {
        let (initiator_type, initiator_id) = if snapshot_key == "_magicmtn_general_agent" {
            ("general_agent", id.to_string())
        } else {
            ("cli", "local:magicmtn".into())
        };
        let affected = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await.map_err(storage_error)?;
                policy.admit(&mut tx, user).await?;
                let result=sqlx::query("INSERT INTO sandboxes(id,provider_id,status,metadata,template,notes,initiator_type,initiator_id,attributed_user_id,ttl_seconds,expires_at) VALUES($1,$2,'creating',$3,$4,$5,$8,$9,$6,$7,now()+make_interval(secs=>$7::double precision)) ON CONFLICT DO NOTHING")
                    .bind(id).bind(format!("pending:{id}")).bind(metadata).bind(&body.template).bind(&body.notes).bind(user).bind(ttl as i32).bind(initiator_type).bind(initiator_id).execute(&mut *tx).await.map_err(storage_error)?.rows_affected();
                tx.commit().await.map_err(storage_error)?;
                result
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => {
                // A host with PostgreSQL policy hooks must never silently bypass them.
                policy.admit_local()?;
                db.reserve(id, body, metadata, ttl, initiator_type, &initiator_id)
                    .await
                    .map_err(storage_error)?
            }
        };
        if affected != 1 {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "creation already attempted; inspect instead of replay".into(),
            ));
        }
        Ok(())
    }
    pub async fn failure(&self, id: Uuid, status: &str, reason: &str) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query("UPDATE sandboxes SET status=$2,failure_reason=$3 WHERE id=$1")
                    .bind(id)
                    .bind(status)
                    .bind(reason)
                    .execute(pool)
                    .await?;
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.failure(id, status, reason).await?,
        }
        Ok(())
    }
    pub async fn attach(&self, id: Uuid, sandbox: &ProviderSandbox, recover: bool) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query("UPDATE sandboxes SET provider_id=$2,status=CASE WHEN status='terminating' THEN status ELSE $3 END,create_time_ms=$4,metadata=CASE WHEN $6 THEN $5||COALESCE(metadata,'{}'::jsonb) ELSE $5 END WHERE id=$1 AND provider_id LIKE 'pending:%'")
                    .bind(id).bind(&sandbox.provider_id).bind(&sandbox.status).bind(sandbox.create_time_ms).bind(&sandbox.metadata).bind(recover).execute(pool).await?;
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.attach(id, sandbox, recover).await?,
        }
        Ok(())
    }
    pub async fn request_termination(&self, id: Uuid, reason: &str) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                crate::sandbox_lifecycle::request_termination(&mut tx, id, reason).await?;
                tx.commit().await?;
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.request_termination(id, reason).await?,
        }
        Ok(())
    }
    pub async fn validate_binding(&self, binding: &Binding, cleanup: bool) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT organization_id,status FROM provider_connections WHERE id=$1",
                )
                .bind(binding.connection_id)
                .fetch_one(pool)
                .await?;
                ensure!(
                    row.get::<Uuid, _>("organization_id") == binding.organization_id
                        && (cleanup || row.get::<String, _>("status") == "enabled"),
                    "connection disabled or wrong organization"
                );
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.validate_binding(binding, cleanup).await?,
        }
        Ok(())
    }
    pub async fn reserve_receipt(
        &self,
        binding: &Binding,
        scope: &str,
        kind: &str,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool)=>Ok(sqlx::query("INSERT INTO provider_resource_receipts(organization_id,scope,kind,binding) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(binding.organization_id).bind(scope).bind(kind).bind(json!(binding)).execute(pool).await?.rows_affected()==1),
            #[cfg(feature="sqlite")]
            Self::Sqlite(db)=>db.reserve_receipt(binding,scope,kind).await,
        }
    }
    pub async fn receipt(&self, binding: &Binding, scope: &str, kind: &str) -> Result<Value> {
        let result:Option<Value>=match self {
            Self::Postgres(pool)=>sqlx::query_scalar("SELECT result FROM provider_resource_receipts WHERE organization_id=$1 AND scope=$2 AND kind=$3 AND binding=$4 AND state='completed'").bind(binding.organization_id).bind(scope).bind(kind).bind(json!(binding)).fetch_optional(pool).await?.flatten(),
            #[cfg(feature="sqlite")]
            Self::Sqlite(db)=>db.receipt(binding,scope,kind).await?,
        };
        result.context("create already dispatched; reconcile instead of replay")
    }
    pub async fn finish_receipt(
        &self,
        binding: &Binding,
        scope: &str,
        kind: &str,
        result: Option<Value>,
    ) -> Result<()> {
        let state = if result.is_some() {
            "completed"
        } else {
            "uncertain"
        };
        match self {
            Self::Postgres(pool) => {
                sqlx::query("UPDATE provider_resource_receipts SET state=$4,result=$5 WHERE organization_id=$1 AND scope=$2 AND kind=$3").bind(binding.organization_id).bind(scope).bind(kind).bind(state).bind(result).execute(pool).await?;
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.finish_receipt(scope, kind, state, result).await?,
        }
        Ok(())
    }
}
fn storage_error(error: impl std::fmt::Display) -> ApiError {
    tracing::error!(%error,"sandbox persistence failed");
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "sandbox operation failed".into(),
    )
}
