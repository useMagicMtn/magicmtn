//! SQLite storage for a single-user installation. All multi-row writes take the
//! writer lock before reading; provider RPCs run after the transaction commits.
use crate::provider_connections::Binding;
use crate::sandboxes::Create;
use anyhow::{ensure, Context, Result};
use magicmtn_provider_framework::host::Registry;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::Row;
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::{path::Path, time::Duration};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./sqlite_migrations");
#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
    pub id: Uuid,
}
impl Database {
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        // Never initialize over an unrelated database.
        let occupied:i64=sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'_sqlx_migrations'").fetch_one(&pool).await?;
        let ours: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='installation'",
        )
        .fetch_one(&pool)
        .await?;
        ensure!(
            occupied == 0 || ours == 1,
            "SQLite database belongs to another application"
        );
        MIGRATOR.run(&pool).await?;
        sqlx::query("INSERT INTO installation(singleton,id) VALUES(1,?) ON CONFLICT DO NOTHING")
            .bind(Uuid::new_v4().to_string())
            .execute(&pool)
            .await?;
        let id: String = sqlx::query_scalar("SELECT id FROM installation WHERE singleton=1")
            .fetch_one(&pool)
            .await?;
        Ok(Self {
            pool,
            id: Uuid::parse_str(&id)?,
        })
    }
    pub async fn bootstrap(&self, registry: &Registry) -> Result<()> {
        self.bootstrap_with_configuration(registry, &Default::default())
            .await
    }

    pub async fn bootstrap_with_configuration(
        &self,
        registry: &Registry,
        overrides: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    ) -> Result<()> {
        for release in registry.list()?.into_iter().filter(|r| {
            r.active
                && r.package.os == std::env::consts::OS
                && r.package.arch == std::env::consts::ARCH
        }) {
            let name = &release.package.name;
            let mut configuration = crate::provider_connections::local_configuration(name);
            if let Some(values) = overrides.get(name) {
                configuration.extend(values.clone());
            }
            let binding = Binding {
                connection_id: Uuid::new_v4(),
                organization_id: self.id,
                generation: 1,
                connection_ref: format!("connection:self-{name}"),
                pin: release.pin(),
                manifest: release.manifest.clone(),
                configuration,
                credential_mode: "managed".into(),
                credential_ref: Some(if release.manifest.credential.is_some() {
                    format!("provider-{name}")
                } else {
                    format!("local:{name}")
                }),
                runner_id: None,
            };
            // Apply startup settings and provider upgrades to future sandboxes.
            // Existing resources retain their original binding and release pin.
            sqlx::query("INSERT INTO provider_connections(id,reference,provider,binding) VALUES(?,?,?,?) ON CONFLICT(reference) DO UPDATE SET binding=json_set(excluded.binding,'$.connection_id',json_extract(provider_connections.binding,'$.connection_id'),'$.generation',json_extract(provider_connections.binding,'$.generation')+1) WHERE json_extract(provider_connections.binding,'$.configuration')<>json_extract(excluded.binding,'$.configuration') OR json_extract(provider_connections.binding,'$.pin')<>json_extract(excluded.binding,'$.pin')").bind(binding.connection_id.to_string()).bind(&binding.connection_ref).bind(name).bind(json!(binding)).execute(&self.pool).await?;
        }
        // Retain and validate frozen releases, including resources awaiting cleanup.
        let mut bindings: Vec<Value> =
            sqlx::query_scalar("SELECT binding FROM provider_connections WHERE status='enabled'")
                .fetch_all(&self.pool)
                .await?;
        bindings.extend(sqlx::query_scalar::<_,Value>("SELECT json_extract(record,'$.metadata._magicmtn_connection') FROM sandboxes WHERE status<>'ended'").fetch_all(&self.pool).await?);
        for value in bindings {
            let binding: Binding = serde_json::from_value(value)?;
            let installed = registry.installed_pin(&binding.pin)?;
            ensure!(
                installed.pin() == binding.pin,
                "retained provider digest differs"
            );
        }
        Ok(())
    }
    pub async fn records(&self, prefix: Option<&str>) -> Result<Vec<Value>> {
        Ok(match prefix {
            Some(prefix) => {
                sqlx::query_scalar("SELECT record FROM sandboxes WHERE id LIKE ? LIMIT 2")
                    .bind(format!("{}%", prefix.to_ascii_lowercase()))
                    .fetch_all(&self.pool)
                    .await?
            }
            None => sqlx::query_scalar(
                "SELECT record FROM sandboxes ORDER BY json_extract(record,'$.created_at') DESC,id",
            )
            .fetch_all(&self.pool)
            .await?,
        })
    }
    pub async fn get(&self, id: Uuid) -> Result<Option<Value>> {
        Ok(
            sqlx::query_scalar("SELECT record FROM sandboxes WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn binding(
        &self,
        id: Option<Uuid>,
        reference: Option<&str>,
        provider: &str,
    ) -> Result<Binding> {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT binding FROM provider_connections WHERE status='enabled' AND ((? IS NOT NULL AND id=?) OR (? IS NULL AND provider=? AND (? IS NULL OR reference=?))) LIMIT 2")
            .bind(id.map(|v|v.to_string())).bind(id.map(|v|v.to_string())).bind(id.map(|v|v.to_string())).bind(provider).bind(reference).bind(reference).fetch_all(&self.pool).await?;
        ensure!(rows.len() == 1, "select an explicit provider connection");
        Ok(serde_json::from_value(rows[0].clone())?)
    }
    pub async fn reserve(
        &self,
        id: Uuid,
        body: &Create,
        metadata: &Value,
        ttl: u32,
        initiator_type: &str,
        initiator_id: &str,
    ) -> Result<u64> {
        let now = chrono::Utc::now();
        let record = json!({"id":id,"provider_id":format!("pending:{id}"),"status":"creating","metadata":metadata,"template":body.template,"notes":body.notes,"initiator_type":initiator_type,"initiator_id":initiator_id,"attributed_user_id":null,"ttl_seconds":ttl,"expires_at":now+chrono::Duration::seconds(ttl.into()),"created_at":now,"create_time_ms":null,"end_time_ms":null,"shutdown_reason":null,"failure_reason":null,"runtime_state":null,"runtime_checked_at":null,"runtime_error":null});
        Ok(
            sqlx::query("INSERT INTO sandboxes(id,record) VALUES(?,?) ON CONFLICT DO NOTHING")
                .bind(id.to_string())
                .bind(record)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }
    pub async fn failure(&self, id: Uuid, status: &str, reason: &str) -> Result<()> {
        sqlx::query("UPDATE sandboxes SET record=json_set(record,'$.status',?,'$.failure_reason',?) WHERE id=?").bind(status).bind(reason).bind(id.to_string()).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn attach(&self, id: Uuid, sandbox: &ProviderSandbox, recover: bool) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut record: Value = sqlx::query_scalar("SELECT record FROM sandboxes WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if record["provider_id"]
            .as_str()
            .is_some_and(|v| v.starts_with("pending:"))
        {
            let mut metadata = sandbox.metadata.clone().unwrap_or_else(|| json!({}));
            if recover {
                if let Some(saved) = record["metadata"].as_object() {
                    for (key, value) in saved {
                        metadata[key] = value.clone();
                    }
                }
            }
            record["provider_id"] = json!(sandbox.provider_id);
            if record["status"] != "terminating" {
                record["status"] = json!(sandbox.status);
            }
            record["create_time_ms"] = json!(sandbox.create_time_ms);
            record["metadata"] = metadata;
            save(&mut tx, id, &record).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn validate_binding(&self, binding: &Binding, cleanup: bool) -> Result<()> {
        let status: String =
            sqlx::query_scalar("SELECT status FROM provider_connections WHERE id=?")
                .bind(binding.connection_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        ensure!(
            binding.organization_id == self.id && (cleanup || status == "enabled"),
            "connection disabled or wrong installation"
        );
        let prefix = if binding.manifest.credential.is_some() {
            "provider-"
        } else {
            "local:"
        };
        ensure!(
            binding.runner_id.is_none()
                && binding
                    .credential_ref
                    .as_deref()
                    .and_then(|reference| reference.strip_prefix(prefix))
                    == Some(binding.pin.name.as_str()),
            "local provider credential required"
        );
        Ok(())
    }
    pub async fn reserve_receipt(
        &self,
        binding: &Binding,
        scope: &str,
        kind: &str,
    ) -> Result<bool> {
        Ok(sqlx::query("INSERT INTO provider_resource_receipts(scope,kind,binding) VALUES(?,?,?) ON CONFLICT DO NOTHING").bind(scope).bind(kind).bind(json!(binding)).execute(&self.pool).await?.rows_affected()==1)
    }
    pub async fn receipt(
        &self,
        binding: &Binding,
        scope: &str,
        kind: &str,
    ) -> Result<Option<Value>> {
        Ok(sqlx::query_scalar("SELECT result FROM provider_resource_receipts WHERE scope=? AND kind=? AND binding=? AND state='completed'").bind(scope).bind(kind).bind(json!(binding)).fetch_optional(&self.pool).await?.flatten())
    }
    pub async fn finish_receipt(
        &self,
        scope: &str,
        kind: &str,
        state: &str,
        result: Option<Value>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE provider_resource_receipts SET state=?,result=? WHERE scope=? AND kind=?",
        )
        .bind(state)
        .bind(result)
        .bind(scope)
        .bind(kind)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn request_termination(&self, id: Uuid, reason: &str) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut record: Value = sqlx::query_scalar("SELECT record FROM sandboxes WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if record["status"] != "ended" {
            record["status"] = json!("terminating");
            if record["shutdown_reason"].is_null() {
                record["shutdown_reason"] = json!(reason);
            }
            save(&mut tx, id, &record).await?;
            sqlx::query("INSERT INTO sandbox_lifecycle_jobs(sandbox_id,operation,provider_pin,provider_scope) VALUES(?,'terminate',?,?) ON CONFLICT(sandbox_id) DO UPDATE SET operation='terminate',completed_at=NULL,next_attempt_at=unixepoch(),attempts=0,last_error=NULL,lease_token=NULL,lease_expires_at=NULL WHERE sandbox_lifecycle_jobs.operation='inspect' OR sandbox_lifecycle_jobs.completed_at IS NOT NULL")
                .bind(id.to_string()).bind(&record["metadata"]["_magicmtn_provider"]).bind(record["metadata"]["_magicmtn_scope"].as_str().context("sandbox scope missing")?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
pub(super) async fn save(tx: &mut Transaction<'_, Sqlite>, id: Uuid, record: &Value) -> Result<()> {
    sqlx::query("UPDATE sandboxes SET record=? WHERE id=?")
        .bind(record)
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

use crate::sandbox_lifecycle::{completion, Job, Observation, LEASE_SECONDS};
use magicmtn_provider_framework::host::Pin;
use magicmtn_sdk::{plugin::Value as ProviderValue, ProviderSandbox};
impl Database {
    pub async fn schedule(&self) -> Result<()> {
        let receipts=sqlx::query("SELECT r.scope,r.binding,r.result FROM provider_resource_receipts r JOIN sandboxes b ON r.scope='sandbox:'||b.id WHERE r.kind='sandbox' AND r.state='completed' AND b.provider_id LIKE 'pending:%' LIMIT 100").fetch_all(&self.pool).await?;
        for row in receipts {
            let scope: String = row.get("scope");
            let id = Uuid::parse_str(
                scope
                    .strip_prefix("sandbox:")
                    .context("invalid sandbox scope")?,
            )?;
            let binding: Binding = serde_json::from_value(row.get("binding"))?;
            let ProviderValue::Sandbox(mut sandbox) = serde_json::from_value(row.get("result"))?
            else {
                continue;
            };
            sandbox.metadata = Some(crate::sandbox_lifecycle::receipt_metadata(
                &sandbox, &binding, &scope,
            ));
            self.attach(id, &sandbox, true).await?;
        }
        let rows:Vec<(String,bool)>=sqlx::query_as("SELECT id,status='terminating' FROM sandboxes b WHERE status<>'ended' AND (status='terminating' OR julianday(expires_at)<=julianday('now')) AND NOT EXISTS(SELECT 1 FROM sandbox_lifecycle_jobs j WHERE j.sandbox_id=b.id AND j.operation='terminate' AND j.completed_at IS NULL) ORDER BY id LIMIT 100").fetch_all(&self.pool).await?;
        for (id, terminating) in rows {
            self.request_termination(
                Uuid::parse_str(&id)?,
                if terminating {
                    "session_ended"
                } else {
                    "sandbox_ttl_expired"
                },
            )
            .await?;
        }
        sqlx::query("INSERT INTO sandbox_lifecycle_jobs(sandbox_id,operation,provider_pin,provider_scope) SELECT id,'inspect',json_extract(record,'$.metadata._magicmtn_provider'),json_extract(record,'$.metadata._magicmtn_scope') FROM sandboxes WHERE status IN ('running','stopped') ON CONFLICT(sandbox_id) DO NOTHING").execute(&self.pool).await?;
        Ok(())
    }
    pub(crate) async fn claim(&self, id: Option<Uuid>) -> Result<Option<Job>> {
        // SQLite has one writer. Acquire it before selecting so concurrent
        // workers cannot claim the same lease (no SKIP LOCKED equivalent).
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let candidate:Option<String>=sqlx::query_scalar("SELECT sandbox_id FROM sandbox_lifecycle_jobs WHERE completed_at IS NULL AND next_attempt_at<=unixepoch() AND (lease_expires_at IS NULL OR lease_expires_at<=unixepoch()) AND (? IS NULL OR sandbox_id=?) ORDER BY CASE operation WHEN 'terminate' THEN 0 ELSE 1 END,next_attempt_at,sandbox_id LIMIT 1")
            .bind(id.map(|v|v.to_string())).bind(id.map(|v|v.to_string())).fetch_optional(&mut *tx).await?;
        let Some(id) = candidate else {
            tx.commit().await?;
            return Ok(None);
        };
        let token = Uuid::new_v4();
        sqlx::query("UPDATE sandbox_lifecycle_jobs SET lease_token=?,lease_expires_at=unixepoch()+?,attempts=attempts+1 WHERE sandbox_id=?").bind(token.to_string()).bind(LEASE_SECONDS).bind(&id).execute(&mut *tx).await?;
        let row=sqlx::query("SELECT j.*,b.record FROM sandbox_lifecycle_jobs j JOIN sandboxes b ON b.id=j.sandbox_id WHERE j.sandbox_id=?").bind(&id).fetch_one(&mut *tx).await?;
        let record: Value = row.get("record");
        let job = Job {
            id: Uuid::parse_str(&id)?,
            token,
            operation: row.get("operation"),
            pin: row
                .get::<Option<Value>, _>("provider_pin")
                .map(serde_json::from_value)
                .transpose()?,
            scope: row.get("provider_scope"),
            attempts: row.get("attempts"),
            sandbox: ProviderSandbox {
                provider_id: record["provider_id"]
                    .as_str()
                    .context("missing provider id")?
                    .into(),
                status: record["status"].as_str().map(str::to_owned),
                create_time_ms: record["create_time_ms"].as_i64(),
                metadata: Some(record["metadata"].clone()),
            },
        };
        tx.commit().await?;
        Ok(Some(job))
    }
    pub(crate) async fn fence_pin(&self, job: &Job, pin: &Pin) -> Result<u64> {
        Ok(sqlx::query("UPDATE sandbox_lifecycle_jobs SET provider_pin=? WHERE sandbox_id=? AND lease_token=? AND lease_expires_at>unixepoch()").bind(json!(pin)).bind(job.id.to_string()).bind(job.token.to_string()).execute(&self.pool).await?.rows_affected())
    }
    pub(crate) async fn finish(&self, job: &Job, result: &Result<Observation>) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let valid:bool=sqlx::query_scalar("SELECT COALESCE(lease_token=? AND lease_expires_at>unixepoch() AND operation=? AND completed_at IS NULL,0) FROM sandbox_lifecycle_jobs WHERE sandbox_id=?").bind(job.token.to_string()).bind(&job.operation).bind(job.id.to_string()).fetch_one(&mut *tx).await?;
        if !valid {
            return Ok(false);
        }
        let mut record: Value = sqlx::query_scalar("SELECT record FROM sandboxes WHERE id=?")
            .bind(job.id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        let outcome = completion(record["status"] == "ended", job.attempts, result);
        if let Some((state, terminal, reason)) = outcome.observation {
            record["status"] = json!(if terminal { "ended" } else { state });
            record["runtime_state"] = json!(state);
            record["runtime_checked_at"] = json!(chrono::Utc::now());
            record["runtime_error"] = Value::Null;
            if terminal {
                if record["end_time_ms"].is_null() {
                    record["end_time_ms"] = json!(chrono::Utc::now().timestamp_millis());
                }
                if record["shutdown_reason"].is_null() {
                    record["shutdown_reason"] = json!(reason);
                }
            }
        }
        if let Some(error) = &outcome.error {
            record["runtime_error"] = json!(error);
        }
        save(&mut tx, job.id, &record).await?;
        sqlx::query("UPDATE sandbox_lifecycle_jobs SET completed_at=CASE WHEN ? THEN unixepoch() ELSE NULL END,lease_token=NULL,lease_expires_at=NULL,next_attempt_at=unixepoch()+?,last_error=?,attempts=CASE WHEN ? IS NULL THEN 0 ELSE attempts END WHERE sandbox_id=? AND lease_token=?")
            .bind(outcome.complete).bind(outcome.delay).bind(&outcome.error).bind(&outcome.error).bind(job.id.to_string()).bind(job.token.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(outcome.complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn leases_fence_late_inspection_and_reclaimed_completion() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let db = Database::open(&directory.path().join("test.sqlite3")).await?;
        let id = Uuid::new_v4();
        let pin: Pin = serde_json::from_value(
            json!({"name":"test-local","version":"0.1.0","sha256":"a".repeat(64)}),
        )?;
        let record = json!({"id":id,"provider_id":"native","status":"running","metadata":{"_magicmtn_provider":pin,"_magicmtn_scope":format!("sandbox:{id}")}});
        sqlx::query("INSERT INTO sandboxes(id,record) VALUES(?,?)")
            .bind(id.to_string())
            .bind(record)
            .execute(&db.pool)
            .await?;
        db.schedule().await?;
        let (one, two) = tokio::try_join!(db.claim(Some(id)), db.claim(Some(id)))?;
        assert_ne!(one.is_some(), two.is_some());
        let inspect = one.or(two).unwrap();
        db.request_termination(id, "user_requested").await?;
        assert_eq!(db.fence_pin(&inspect, &pin).await?, 0);
        assert!(
            !db.finish(
                &inspect,
                &Ok(Observation::State(magicmtn_sdk::SandboxState::Running))
            )
            .await?
        );
        assert_eq!(db.get(id).await?.unwrap()["status"], "terminating");
        let terminate = db.claim(Some(id)).await?.unwrap();
        db.request_termination(id, "repeat").await?;
        assert!(db.claim(Some(id)).await?.is_none());
        sqlx::query(
            "UPDATE sandbox_lifecycle_jobs SET lease_expires_at=unixepoch()-1 WHERE sandbox_id=?",
        )
        .bind(id.to_string())
        .execute(&db.pool)
        .await?;
        assert!(!db.finish(&terminate, &Ok(Observation::Terminated)).await?);
        let replacement = db.claim(Some(id)).await?.unwrap();
        assert!(!db.finish(&terminate, &Ok(Observation::Terminated)).await?);
        assert!(
            db.finish(&replacement, &Ok(Observation::Terminated))
                .await?
        );
        let saved = db.get(id).await?.unwrap();
        assert_eq!(saved["status"], "ended");
        assert_eq!(saved["shutdown_reason"], "user_requested");
        db.pool.close().await;
        Ok(())
    }
    #[tokio::test]
    async fn provider_failure_preserves_observation_and_retries() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let db = Database::open(&directory.path().join("test.sqlite3")).await?;
        let id = Uuid::new_v4();
        let record = json!({"id":id,"provider_id":"native","status":"running","runtime_state":"running","runtime_checked_at":"2026-01-01T00:00:00Z","metadata":{"_magicmtn_scope":format!("sandbox:{id}")}});
        sqlx::query("INSERT INTO sandboxes(id,record) VALUES(?,?)")
            .bind(id.to_string())
            .bind(record)
            .execute(&db.pool)
            .await?;
        db.schedule().await?;
        let job = db.claim(Some(id)).await?.unwrap();
        assert!(
            !db.finish(&job, &Err(anyhow::anyhow!("provider unavailable")))
                .await?
        );
        let saved = db.get(id).await?.unwrap();
        assert_eq!(saved["status"], "running");
        assert_eq!(saved["runtime_checked_at"], "2026-01-01T00:00:00Z");
        assert_eq!(saved["runtime_error"], "provider unavailable");
        assert!(db.claim(Some(id)).await?.is_none());
        sqlx::query(
            "UPDATE sandbox_lifecycle_jobs SET next_attempt_at=unixepoch()-1 WHERE sandbox_id=?",
        )
        .bind(id.to_string())
        .execute(&db.pool)
        .await?;
        let job = db.claim(Some(id)).await?.unwrap();
        assert!(
            db.finish(
                &job,
                &Ok(Observation::State(magicmtn_sdk::SandboxState::Missing))
            )
            .await?
        );
        let saved = db.get(id).await?.unwrap();
        assert_eq!(saved["status"], "ended");
        assert_eq!(saved["shutdown_reason"], "runtime_missing");
        db.pool.close().await;
        Ok(())
    }
}
