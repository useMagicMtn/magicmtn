//! Durable sandbox cleanup and authoritative runtime reconciliation.
//! Database leases fence workers; provider calls never run inside a transaction.
use crate::sandbox_storage::Store;
use anyhow::{ensure, Context, Result};
use magicmtn_provider_framework::host::{Pin, Registry};
use magicmtn_sdk::{
    plugin::{Operation, SandboxOperation, Value as ProviderValue},
    BoxFuture, ProviderSandbox, SandboxState,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::time::Duration;
use uuid::Uuid;

pub(crate) const INSPECTION_SECONDS: i32 = 60;
pub(crate) const LEASE_SECONDS: i32 = 120;
const CALL_SECONDS: u64 = 30;

/// Lock sessions before the sandbox everywhere, including worker completion.
async fn lock_sandbox(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Value> {
    sqlx::query("SELECT id FROM chat_sessions WHERE sandbox_id=$1 ORDER BY id FOR UPDATE")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT to_jsonb(b) FROM sandboxes b WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut **tx)
        .await
        .context("sandbox not found")
}

async fn enqueue(tx: &mut Transaction<'_, Postgres>, id: Uuid, operation: &str) -> Result<()> {
    sqlx::query("INSERT INTO sandbox_lifecycle_jobs(organization_id,sandbox_id,operation,provider_pin,provider_scope) SELECT b.organization_id,b.id,$2,COALESCE(NULLIF(b.metadata->'_magicmtn_provider','null'::jsonb),NULLIF(s.provider_bindings->'sandbox','null'::jsonb)),COALESCE(b.metadata->>'_magicmtn_scope',s.id::text,b.id::text) FROM sandboxes b LEFT JOIN LATERAL (SELECT id,provider_bindings FROM chat_sessions WHERE sandbox_id=b.id ORDER BY id LIMIT 1) s ON true WHERE b.id=$1 ON CONFLICT(sandbox_id) DO UPDATE SET operation='terminate',completed_at=NULL,next_attempt_at=now(),attempts=0,last_error=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=now() WHERE sandbox_lifecycle_jobs.operation='inspect' OR sandbox_lifecycle_jobs.completed_at IS NOT NULL")
        .bind(id).bind(operation).execute(&mut **tx).await?;
    Ok(())
}

/// Called in the same transaction that ends a session or fences attached work.
/// Repeated requests leave an in-flight termination lease intact.
pub async fn request_termination(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    reason: &str,
) -> Result<()> {
    let record = lock_sandbox(tx, id).await?;
    if record["status"] == "ended" {
        return Ok(());
    }
    sqlx::query("UPDATE chat_turns t SET state='stopped',terminal_reason=$2,lease_token=NULL,updated_at=now() FROM chat_sessions s WHERE t.session_id=s.id AND s.sandbox_id=$1 AND t.state IN ('queued','running')")
        .bind(id).bind(reason).execute(&mut **tx).await?;
    sqlx::query("UPDATE chat_sessions SET state='ended',updated_at=now() WHERE sandbox_id=$1 AND state<>'ended'")
        .bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE sandboxes SET status='terminating',shutdown_reason=COALESCE(shutdown_reason,$2) WHERE id=$1")
        .bind(id).bind(reason).execute(&mut **tx).await?;
    enqueue(tx, id, "terminate").await
}

/// Recover pre-existing ended sessions and interrupted termination requests.
/// Also enroll active resources for inspection after startup.
pub async fn schedule(pool: &PgPool) -> Result<()> {
    recover_received_resources(pool).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT b.id FROM sandboxes b WHERE b.status IS DISTINCT FROM 'ended' AND NOT EXISTS(SELECT 1 FROM sandbox_lifecycle_jobs j WHERE j.sandbox_id=b.id AND j.operation='terminate' AND j.completed_at IS NULL) AND (b.status='terminating' OR EXISTS(SELECT 1 FROM chat_sessions s WHERE s.sandbox_id=b.id AND s.state='ended')) ORDER BY b.id LIMIT 100")
        .fetch_all(pool).await?;
    for id in ids {
        let mut tx = pool.begin().await?;
        request_termination(&mut tx, id, "session_ended").await?;
        tx.commit().await?;
    }
    schedule_expiry(pool).await?;
    sqlx::query("INSERT INTO sandbox_lifecycle_jobs(organization_id,sandbox_id,operation,provider_pin,provider_scope) SELECT b.organization_id,b.id,'inspect',COALESCE(NULLIF(b.metadata->'_magicmtn_provider','null'::jsonb),NULLIF(s.provider_bindings->'sandbox','null'::jsonb)),COALESCE(b.metadata->>'_magicmtn_scope',s.id::text,b.id::text) FROM sandboxes b LEFT JOIN LATERAL (SELECT id,provider_bindings FROM chat_sessions WHERE sandbox_id=b.id ORDER BY id LIMIT 1) s ON true WHERE b.status IN ('running','stopped') ON CONFLICT(sandbox_id) DO NOTHING")
        .execute(pool).await?;
    Ok(())
}

async fn schedule_expiry(pool: &PgPool) -> Result<()> {
    let expired: Vec<(Uuid, bool)> = sqlx::query_as("SELECT b.id,b.expires_at IS NOT NULL FROM sandboxes b WHERE b.status IS DISTINCT FROM 'ended' AND ((b.expires_at IS NOT NULL AND b.expires_at<=now()) OR (b.expires_at IS NULL AND b.created_at<now()-interval '24 hours' AND b.metadata#>>'{_magicmtn_connection,connection_ref}' LIKE 'connection:cloud-%')) AND NOT EXISTS(SELECT 1 FROM sandbox_lifecycle_jobs j WHERE j.sandbox_id=b.id AND j.operation='terminate' AND j.completed_at IS NULL) ORDER BY b.expires_at NULLS LAST,b.created_at,b.id LIMIT 100")
        .fetch_all(pool).await?;
    for (id, has_ttl) in expired {
        let mut tx = pool.begin().await?;
        request_termination(
            &mut tx,
            id,
            if has_ttl {
                "sandbox_ttl_expired"
            } else {
                "cloud_lifetime_limit"
            },
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// A create response can arrive durably before its caller attaches the sandbox.
/// Reconnect it after a restart, including when cancellation won the race.
pub async fn recover_received_resources(pool: &PgPool) -> Result<()> {
    // The polling runner commits a response independently of the waiting backend.
    sqlx::query("UPDATE provider_resource_receipts r SET state='completed',result=j.result->'ok' FROM provider_rpc_jobs j WHERE j.organization_id=r.organization_id AND j.scope=r.scope AND j.binding=r.binding AND j.state='completed' AND r.state<>'completed' AND j.result ? 'ok' AND ((r.kind='sandbox' AND j.operation->>'name'='sandbox_create') OR (r.kind='inference' AND j.operation->>'name'='inference_deployment_create'))")
        .execute(pool).await?;
    let inference=sqlx::query("SELECT i.id,r.result FROM inference_resources i JOIN provider_resource_receipts r ON r.organization_id=i.organization_id AND r.scope='inference:'||i.id::text AND r.kind='inference' AND r.binding=i.binding WHERE i.provider_id IS NULL AND r.state='completed'")
        .fetch_all(pool).await?;
    for row in inference {
        if let ProviderValue::InferenceDeployment(snapshot) =
            serde_json::from_value(row.get("result"))?
        {
            let state = match snapshot.state {
                magicmtn_sdk::InferenceDeploymentState::Stopped => "stopped",
                magicmtn_sdk::InferenceDeploymentState::Deleted => "deleted",
                magicmtn_sdk::InferenceDeploymentState::Failed => "failed",
                _ => "active",
            };
            sqlx::query("UPDATE inference_resources SET provider_id=$2,snapshot=$3,state=$4,last_error=NULL WHERE id=$1 AND provider_id IS NULL")
                .bind(row.get::<Uuid,_>("id")).bind(&snapshot.resource.provider_id).bind(serde_json::json!(snapshot)).bind(state).execute(pool).await?;
        }
    }
    let receipts=sqlx::query("SELECT r.organization_id,r.scope,r.binding,r.result FROM provider_resource_receipts r WHERE r.kind='sandbox' AND r.state='completed' AND (EXISTS(SELECT 1 FROM chat_sessions s WHERE 'session:'||s.id::text=r.scope AND s.organization_id=r.organization_id AND s.sandbox_id IS NULL) OR EXISTS(SELECT 1 FROM sandboxes b WHERE 'sandbox:'||b.id::text=r.scope AND b.organization_id=r.organization_id AND b.provider_id LIKE 'pending:%')) LIMIT 100")
        .fetch_all(pool).await?;
    for receipt in receipts {
        let scope: String = receipt.get("scope");
        let (source, raw_id) = scope.split_once(':').context("resource scope missing")?;
        let id = Uuid::parse_str(raw_id)?;
        let binding: crate::provider_connections::Binding =
            serde_json::from_value(receipt.get("binding"))?;
        let ProviderValue::Sandbox(sandbox) = serde_json::from_value(receipt.get("result"))? else {
            continue;
        };
        let mut tx = pool.begin().await?;
        crate::workspace::set_transaction(&mut tx, binding.organization_id).await?;
        let session=sqlx::query("SELECT s.sandbox_id,s.selected_compute_id,s.compute_snapshot,s.sandbox_ttl_seconds,s.sandbox_expires_at,a.manifest FROM chat_sessions s JOIN artifact_manifests a ON a.revision_id=s.revision_id WHERE s.id=$1 AND $2='session' FOR UPDATE OF s")
            .bind(id).bind(source).fetch_optional(&mut *tx).await?;
        let mut metadata = receipt_metadata(&sandbox, &binding, &scope);
        if let Some(session) = session {
            if session.get::<Option<Uuid>, _>("sandbox_id").is_none() {
                let sandbox_id = Uuid::new_v4();
                let compute: Option<Value> = session.get("compute_snapshot");
                let artifact: Value = session.get("manifest");
                let image = artifact["runtime_configuration"]["sandbox_image"]
                    .as_str()
                    .context("frozen image missing")?;
                metadata["image"] = serde_json::json!(image);
                sqlx::query("INSERT INTO sandboxes(id,provider_id,status,create_time_ms,metadata,template,initiator_type,initiator_id,compute_id,compute_binding_ref,ttl_seconds,expires_at) VALUES($1,$2,'running',$3,$4,$5,'chat_session',$6,$7,$8,$9,$10)")
                    .bind(sandbox_id).bind(&sandbox.provider_id).bind(sandbox.create_time_ms).bind(&metadata).bind(image).bind(id.to_string())
                    .bind(session.get::<Option<Uuid>,_>("selected_compute_id")).bind(compute.as_ref().and_then(|c|c["binding_ref"].as_str())).bind(session.get::<Option<i32>,_>("sandbox_ttl_seconds")).bind(session.get::<Option<chrono::DateTime<chrono::Utc>>,_>("sandbox_expires_at")).execute(&mut *tx).await?;
                sqlx::query(
                    "UPDATE chat_sessions SET sandbox_id=$2 WHERE id=$1 AND sandbox_id IS NULL",
                )
                .bind(id)
                .bind(sandbox_id)
                .execute(&mut *tx)
                .await?;
            }
        } else {
            sqlx::query("UPDATE sandboxes SET provider_id=$2,status=CASE WHEN status='terminating' THEN status ELSE $3 END,create_time_ms=$4,metadata=$5||COALESCE(metadata,'{}'::jsonb) WHERE id=$1 AND provider_id LIKE 'pending:%'")
                .bind(id).bind(&sandbox.provider_id).bind(&sandbox.status).bind(sandbox.create_time_ms).bind(metadata).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) id: Uuid,
    pub(crate) operation: String,
    pub(crate) pin: Option<Pin>,
    pub(crate) scope: String,
    pub(crate) token: Uuid,
    pub(crate) attempts: i32,
    pub(crate) sandbox: ProviderSandbox,
}

async fn claim(pool: &PgPool, id: Option<Uuid>) -> Result<Option<Job>> {
    let token = Uuid::new_v4();
    let row = sqlx::query("WITH candidate AS (SELECT sandbox_id FROM sandbox_lifecycle_jobs WHERE completed_at IS NULL AND next_attempt_at<=now() AND (lease_expires_at IS NULL OR lease_expires_at<=now()) AND ($1::uuid IS NULL OR sandbox_id=$1) ORDER BY CASE operation WHEN 'terminate' THEN 0 ELSE 1 END,next_attempt_at,sandbox_id FOR UPDATE SKIP LOCKED LIMIT 1), claimed AS (UPDATE sandbox_lifecycle_jobs j SET lease_token=$2,lease_expires_at=now()+make_interval(secs=>$3),attempts=attempts+1,updated_at=now() FROM candidate c WHERE j.sandbox_id=c.sandbox_id RETURNING j.*) SELECT c.*,b.provider_id,b.status,b.create_time_ms,b.metadata FROM claimed c JOIN sandboxes b ON b.id=c.sandbox_id")
        .bind(id).bind(token).bind(LEASE_SECONDS as f64).fetch_optional(pool).await?;
    row.map(|r| {
        Ok(Job {
            id: r.get("sandbox_id"),
            operation: r.get("operation"),
            pin: r
                .get::<Option<Value>, _>("provider_pin")
                .map(serde_json::from_value)
                .transpose()?,
            scope: r.get("provider_scope"),
            token,
            attempts: r.get("attempts"),
            sandbox: ProviderSandbox {
                provider_id: r.get("provider_id"),
                status: r.get("status"),
                create_time_ms: r.get("create_time_ms"),
                metadata: r.get("metadata"),
            },
        })
    })
    .transpose()
}

/// Narrow adapter also used by fault-injection tests without real containers.
pub trait LifecycleProvider: Sync {
    fn resolve_pin(&self, name: &str, version: Option<&str>) -> Result<Pin>;
    fn supports_status(&self, pin: &Pin) -> Result<bool>;
    fn call<'a>(
        &'a self,
        pin: &'a Pin,
        scope: &'a str,
        operation: Operation,
    ) -> BoxFuture<'a, ProviderValue>;
    fn release<'a>(&'a self, scope: &'a str) -> BoxFuture<'a, ()>;
    fn call_bound<'a>(
        &'a self,
        _store: &'a Store,
        binding: Option<&'a crate::provider_connections::Binding>,
        pin: &'a Pin,
        scope: &'a str,
        operation: Operation,
    ) -> BoxFuture<'a, ProviderValue> {
        let _ = binding;
        self.call(pin, scope, operation)
    }
}
impl LifecycleProvider for Registry {
    fn call_bound<'a>(
        &'a self,
        store: &'a Store,
        binding: Option<&'a crate::provider_connections::Binding>,
        pin: &'a Pin,
        scope: &'a str,
        operation: Operation,
    ) -> BoxFuture<'a, ProviderValue> {
        Box::pin(async move {
            match binding {
                Some(binding) => {
                    crate::provider_connections::dispatch_store(
                        store,
                        self,
                        binding,
                        scope,
                        operation,
                        Duration::from_secs(CALL_SECONDS),
                        true,
                    )
                    .await
                }
                None => {
                    self.call(pin, scope, operation, Duration::from_secs(CALL_SECONDS))
                        .await
                }
            }
        })
    }
    fn resolve_pin(&self, name: &str, version: Option<&str>) -> Result<Pin> {
        Ok(match version {
            Some(v) => self.installed(name, v)?,
            None => self.active(name)?,
        }
        .pin())
    }
    fn supports_status(&self, pin: &Pin) -> Result<bool> {
        let installed = self.installed_pin(pin)?;
        ensure!(
            installed.package.sha256 == pin.sha256,
            "provider pin changed"
        );
        Ok(installed
            .manifest
            .capabilities
            .sandbox_operations
            .contains(&SandboxOperation::Status))
    }
    fn call<'a>(
        &'a self,
        pin: &'a Pin,
        scope: &'a str,
        operation: Operation,
    ) -> BoxFuture<'a, ProviderValue> {
        Box::pin(async move {
            self.call(pin, scope, operation, Duration::from_secs(CALL_SECONDS))
                .await
        })
    }
    fn release<'a>(&'a self, scope: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.release(scope).await;
            Ok(())
        })
    }
}

pub(crate) enum Observation {
    Terminated,
    State(SandboxState),
}

async fn execute(
    store: &Store,
    provider: &impl LifecycleProvider,
    job: &Job,
) -> Result<Observation> {
    ensure!(
        !job.sandbox.provider_id.starts_with("pending:"),
        "awaiting_sandbox_create_response"
    );
    let metadata = job.sandbox.metadata.as_ref();
    let binding = metadata
        .and_then(|m| m.get("_magicmtn_connection"))
        .map(|v| serde_json::from_value::<crate::provider_connections::Binding>(v.clone()))
        .transpose()?;
    let pin = match &job.pin {
        Some(pin) => pin.clone(),
        None => provider.resolve_pin(
            metadata
                .and_then(|m| m["provider"].as_str())
                .unwrap_or("apple-container"),
            metadata.and_then(|m| m["backend_version"].as_str()),
        )?,
    };
    // Resolve a legacy reference once, before any external side effect.
    store.fence_pin(job, &pin).await?;
    let supports_status = match &binding {
        Some(binding) => binding
            .manifest
            .capabilities
            .sandbox_operations
            .contains(&SandboxOperation::Status),
        None => provider.supports_status(&pin)?,
    };
    // Fresh maintenance connections allow recovery from ambiguous RPC failures
    // without releasing a live harness connection or replaying harness work.
    let scope = format!("lifecycle:{}:{}", job.id, job.token);
    let result = async {
        if job.operation == "inspect" {
            ensure!(supports_status, "provider_status_unsupported");
            match provider
                .call_bound(
                    store,
                    binding.as_ref(),
                    &pin,
                    &scope,
                    Operation::SandboxStatus {
                        sandbox: job.sandbox.clone(),
                    },
                )
                .await?
            {
                ProviderValue::SandboxStatus(state) => Ok(Observation::State(state)),
                _ => anyhow::bail!("invalid sandbox status response"),
            }
        } else {
            if supports_status {
                match provider
                    .call_bound(
                        store,
                        binding.as_ref(),
                        &pin,
                        &scope,
                        Operation::SandboxStatus {
                            sandbox: job.sandbox.clone(),
                        },
                    )
                    .await?
                {
                    ProviderValue::SandboxStatus(SandboxState::Missing) => {
                        return Ok(Observation::Terminated)
                    }
                    ProviderValue::SandboxStatus(_) => {}
                    _ => anyhow::bail!("invalid sandbox status response"),
                }
            }
            match provider
                .call_bound(
                    store,
                    binding.as_ref(),
                    &pin,
                    &scope,
                    Operation::SandboxTerminate {
                        sandbox: job.sandbox.clone(),
                    },
                )
                .await?
            {
                ProviderValue::Done => Ok(Observation::Terminated),
                _ => anyhow::bail!("invalid sandbox termination response"),
            }
        }
    }
    .await;
    let _ = provider.release(&scope).await;
    result
}

async fn finish(pool: &PgPool, job: &Job, result: &Result<Observation>) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let record = lock_sandbox(&mut tx, job.id).await?;
    // Lock and validate together: a reclaimed lease cannot change between
    // validation and persistence, even if completion waits on a row lock.
    let valid: bool = sqlx::query_scalar("SELECT COALESCE(lease_token=$2 AND lease_expires_at>clock_timestamp() AND operation=$3 AND completed_at IS NULL,false) FROM sandbox_lifecycle_jobs WHERE sandbox_id=$1 FOR UPDATE")
        .bind(job.id).bind(job.token).bind(&job.operation).fetch_one(&mut *tx).await?;
    if !valid {
        return Ok(false);
    }
    let outcome = completion(record["status"] == "ended", job.attempts, result);
    let Completion {
        complete,
        delay,
        error,
        observation,
    } = outcome;
    if let Some((state, terminal, reason)) = observation {
        sqlx::query("UPDATE sandboxes SET status=$2,runtime_state=$3,runtime_checked_at=now(),runtime_error=NULL,end_time_ms=CASE WHEN $4 THEN COALESCE(end_time_ms,(extract(epoch FROM now())*1000)::bigint) ELSE end_time_ms END,shutdown_reason=CASE WHEN $4 THEN COALESCE(shutdown_reason,$5) ELSE shutdown_reason END WHERE id=$1")
            .bind(job.id).bind(if terminal { "ended" } else { state }).bind(state).bind(terminal).bind(reason).execute(&mut *tx).await?;
        if state != "running" {
            sqlx::query("UPDATE chat_turns t SET state='stopped',terminal_reason=$2,lease_token=NULL,updated_at=now() FROM chat_sessions s WHERE t.session_id=s.id AND s.sandbox_id=$1 AND t.state IN ('queued','running')")
                .bind(job.id).bind(reason).execute(&mut *tx).await?;
            sqlx::query("UPDATE chat_sessions SET state='interrupted',updated_at=now() WHERE sandbox_id=$1 AND state<>'ended' AND state<>'interrupted'")
                .bind(job.id).execute(&mut *tx).await?;
        }
    }
    if let Some(error) = &error {
        sqlx::query("UPDATE sandboxes SET runtime_error=$2 WHERE id=$1")
            .bind(job.id)
            .bind(error)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE sandbox_lifecycle_jobs SET completed_at=CASE WHEN $3 THEN now() ELSE NULL END,lease_token=NULL,lease_expires_at=NULL,next_attempt_at=now()+make_interval(secs=>$4),last_error=$5,attempts=CASE WHEN $5::text IS NULL THEN 0 ELSE attempts END,updated_at=now() WHERE sandbox_id=$1 AND lease_token=$2")
        .bind(job.id).bind(job.token).bind(complete).bind(delay as f64).bind(error).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(complete)
}

/// Process one due job. Optional ID supports a best-effort synchronous CLI
/// termination; a leased job is left to its current worker.
pub async fn run_one(
    pool: &PgPool,
    provider: &impl LifecycleProvider,
    id: Option<Uuid>,
) -> Result<bool> {
    run_one_store(&pool.clone().into(), provider, id).await
}

pub async fn run_one_store(
    store: &Store,
    provider: &impl LifecycleProvider,
    id: Option<Uuid>,
) -> Result<bool> {
    let Some(job) = store.claim(id).await? else {
        return Ok(false);
    };
    // Bound the whole attempt, including connection startup, not only RPC calls.
    let result =
        match tokio::time::timeout(Duration::from_secs(75), execute(store, provider, &job)).await {
            Ok(result) => result,
            Err(_) => {
                let _ = provider
                    .release(&format!("lifecycle:{}:{}", job.id, job.token))
                    .await;
                Err(anyhow::anyhow!("sandbox lifecycle attempt timed out"))
            }
        };
    if let Err(error) = &result {
        tracing::warn!(sandbox_id=%job.id,operation=%job.operation,%error,"sandbox lifecycle attempt deferred");
    }
    if store.finish(&job, &result).await? {
        // In-flight harness RPCs are fenced, but can still hold a client lock.
        // Finishing a cleanup request must not wait on an entire model call.
        let _ = tokio::time::timeout(Duration::from_secs(5), provider.release(&job.scope)).await;
    }
    Ok(true)
}

pub async fn run(pool: PgPool, registry: Registry) {
    run_store(pool.into(), registry).await
}

pub async fn run_store(store: Store, registry: Registry) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut workers = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(error) = store.schedule().await {
                    tracing::error!(%error,"sandbox lifecycle scheduling failed");
                    continue;
                }
                // Bound provider concurrency without letting an unavailable
                // backend delay scheduling and recovery for every other one.
                while workers.len() < 4 {
                    let store = store.clone();
                    let registry = registry.clone();
                    workers.spawn(async move { run_one_store(&store, &registry, None).await });
                }
            },
            result = workers.join_next(), if !workers.is_empty() => {
                match result {
                    Some(Ok(Ok(_))) => {},
                    Some(Ok(Err(error))) => tracing::error!(%error,"sandbox lifecycle worker failed"),
                    Some(Err(error)) => tracing::error!(%error,"sandbox lifecycle worker exited"),
                    None => {},
                }
            }
        }
    }
}

/// The lifecycle decision is shared; adapters only persist it atomically with
/// lease validation and any host-specific attached session updates.
pub(crate) struct Completion {
    pub complete: bool,
    pub delay: i32,
    pub error: Option<String>,
    pub observation: Option<(&'static str, bool, &'static str)>,
}
pub(crate) fn completion(ended: bool, attempts: i32, result: &Result<Observation>) -> Completion {
    let mut out = Completion {
        complete: ended,
        delay: INSPECTION_SECONDS,
        error: None,
        observation: None,
    };
    if ended {
        return out;
    }
    match result {
        Ok(observation) => {
            let observation = match observation {
                Observation::Terminated => ("missing", true, "sandbox_terminated"),
                Observation::State(SandboxState::Missing) => ("missing", true, "runtime_missing"),
                Observation::State(SandboxState::Stopped) => ("stopped", false, "runtime_stopped"),
                Observation::State(SandboxState::Running) => ("running", false, ""),
            };
            out.complete = observation.1;
            out.observation = Some(observation);
        }
        Err(error) => {
            out.error = Some(error.to_string().chars().take(1000).collect());
            out.delay = (2_i32.saturating_pow(attempts.min(9) as u32)).clamp(2, 300);
        }
    }
    out
}
impl Store {
    pub async fn schedule(&self) -> Result<()> {
        match self {
            Self::Postgres(pool) => schedule(pool).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.schedule().await,
        }
    }
    async fn claim(&self, id: Option<Uuid>) -> Result<Option<Job>> {
        match self {
            Self::Postgres(pool) => claim(pool, id).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.claim(id).await,
        }
    }
    async fn fence_pin(&self, job: &Job, pin: &Pin) -> Result<()> {
        let affected=match self {
            Self::Postgres(pool)=>sqlx::query("UPDATE sandbox_lifecycle_jobs SET provider_pin=$3 WHERE sandbox_id=$1 AND lease_token=$2 AND lease_expires_at>clock_timestamp()").bind(job.id).bind(job.token).bind(serde_json::to_value(pin)?).execute(pool).await?.rows_affected(),
            #[cfg(feature="sqlite")]
            Self::Sqlite(db)=>db.fence_pin(job,pin).await?,
        };
        ensure!(affected == 1, "lifecycle lease fenced");
        Ok(())
    }
    async fn finish(&self, job: &Job, result: &Result<Observation>) -> Result<bool> {
        match self {
            Self::Postgres(pool) => finish(pool, job, result).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(db) => db.finish(job, result).await,
        }
    }
}

pub(crate) fn receipt_metadata(
    sandbox: &ProviderSandbox,
    binding: &crate::provider_connections::Binding,
    scope: &str,
) -> Value {
    let mut metadata = sandbox
        .metadata
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    metadata["provider"] = serde_json::json!(binding.pin.name);
    metadata["backend_version"] = serde_json::json!(binding.pin.version);
    metadata["_magicmtn_connection"] = serde_json::json!(binding);
    metadata["_magicmtn_provider"] = serde_json::json!(binding.pin);
    metadata["_magicmtn_scope"] = serde_json::json!(scope);
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    struct Fake {
        state: Mutex<SandboxState>,
        unavailable: bool,
        supports_status: bool,
        removals: AtomicUsize,
    }
    impl Fake {
        fn new(state: SandboxState) -> Self {
            Self {
                state: Mutex::new(state),
                unavailable: false,
                supports_status: true,
                removals: AtomicUsize::new(0),
            }
        }
    }
    fn pin() -> Pin {
        Pin {
            name: "test".into(),
            version: "0.1.0".into(),
            sha256: "0".repeat(64),
        }
    }
    impl LifecycleProvider for Fake {
        fn resolve_pin(&self, _: &str, _: Option<&str>) -> Result<Pin> {
            Ok(pin())
        }
        fn supports_status(&self, _: &Pin) -> Result<bool> {
            Ok(self.supports_status)
        }
        fn call<'a>(
            &'a self,
            _: &'a Pin,
            _: &'a str,
            operation: Operation,
        ) -> BoxFuture<'a, ProviderValue> {
            Box::pin(async move {
                ensure!(!self.unavailable, "runtime unavailable");
                match operation {
                    Operation::SandboxStatus { .. } => {
                        Ok(ProviderValue::SandboxStatus(*self.state.lock().unwrap()))
                    }
                    Operation::SandboxTerminate { .. } => {
                        self.removals.fetch_add(1, Ordering::SeqCst);
                        *self.state.lock().unwrap() = SandboxState::Missing;
                        Ok(ProviderValue::Done)
                    }
                    _ => anyhow::bail!("unexpected operation"),
                }
            })
        }
        fn release<'a>(&'a self, _: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }
    async fn migrate(pool: &PgPool) -> Result<()> {
        let directory = std::env::var("MAGICMTN_TEST_MIGRATIONS")
            .context("MAGICMTN_TEST_MIGRATIONS must identify the database migration directory")?;
        let host = sqlx::migrate::Migrator::new(std::path::Path::new(&directory)).await?;
        crate::schema::compose(host.iter().cloned())
            .run(pool)
            .await?;
        Ok(())
    }

    async fn sandbox(pool: &PgPool) -> Result<Uuid> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sandboxes(id,provider_id,status,metadata) VALUES($1,$2,'running',$3)",
        )
        .bind(id)
        .bind(format!("test-{id}"))
        .bind(json!({"provider":"test","_magicmtn_provider":pin()}))
        .execute(pool)
        .await?;
        Ok(id)
    }
    async fn session(pool: &PgPool, sandbox: Uuid) -> Result<(Uuid, Uuid)> {
        let h = Uuid::new_v4();
        let r = Uuid::new_v4();
        let d = Uuid::new_v4();
        let s = Uuid::new_v4();
        let t = Uuid::new_v4();
        sqlx::query("INSERT INTO harnesses(id,name,workflow) VALUES($1,'Lifecycle','interactive')")
            .bind(h)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO harness_revisions(id,harness_id,name,model,status,surfaces,tool_manifest) VALUES($1,$2,'Lifecycle','test','draft','{}','[]')").bind(r).bind(h).execute(pool).await?;
        sqlx::query("INSERT INTO harness_deployments(id,harness_id,revision_id,name) VALUES($1,$2,$3,'test')").bind(d).bind(h).bind(r).execute(pool).await?;
        sqlx::query("INSERT INTO chat_sessions(id,created_by,first_key,first_digest,harness_id,deployment_id,revision_id,sandbox_id,state,provider_bindings) VALUES($1,'local:magicmtn',$1::text,'test',$2,$3,$4,$5,'ready',$6)")
            .bind(s).bind(h).bind(d).bind(r).bind(sandbox).bind(json!({"sandbox":pin()})).execute(pool).await?;
        sqlx::query("INSERT INTO chat_turns(id,session_id,sequence,client_key,prompt_digest,state,lease_token,lease_expires_at) VALUES($1,$2,1,'test','test','running',$3,now()+interval '1 minute')")
            .bind(t).bind(s).bind(Uuid::new_v4()).execute(pool).await?;
        Ok((s, t))
    }
    async fn request(pool: &PgPool, id: Uuid) -> Result<()> {
        let mut tx = pool.begin().await?;
        request_termination(&mut tx, id, "sandbox_terminated").await?;
        tx.commit().await?;
        Ok(())
    }
    async fn status(pool: &PgPool, id: Uuid) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT status FROM sandboxes WHERE id=$1")
                .bind(id)
                .fetch_one(pool)
                .await?,
        )
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "requires isolated DATABASE_URL and MAGICMTN_TEST_MIGRATIONS"]
    async fn expiry_terminates_ttl_sandboxes_and_legacy_cloud_sandboxes(
        pool: PgPool,
    ) -> Result<()> {
        migrate(&pool).await?;
        let old_cloud = sandbox(&pool).await?;
        let recent_cloud = sandbox(&pool).await?;
        let old_local = sandbox(&pool).await?;
        let expired_local = sandbox(&pool).await?;
        let live_local = sandbox(&pool).await?;
        sqlx::query("UPDATE sandboxes SET ttl_seconds=1800,expires_at=CASE WHEN id=$1 THEN now()-interval '1 second' ELSE now()+interval '1 hour' END WHERE id=$1 OR id=$2")
            .bind(expired_local).bind(live_local).execute(&pool).await?;
        for id in [old_cloud, recent_cloud] {
            sqlx::query("UPDATE sandboxes SET metadata=jsonb_set(metadata,'{_magicmtn_connection}', $2::jsonb) WHERE id=$1")
                .bind(id)
                .bind(json!({"connection_ref":"connection:cloud-modal"}))
                .execute(&pool)
                .await?;
        }
        sqlx::query(
            "UPDATE sandboxes SET created_at=now()-interval '25 hours' WHERE id=$1 OR id=$2",
        )
        .bind(old_cloud)
        .bind(old_local)
        .execute(&pool)
        .await?;
        schedule_expiry(&pool).await?;
        assert_eq!(status(&pool, old_cloud).await?, "terminating");
        assert_eq!(status(&pool, recent_cloud).await?, "running");
        assert_eq!(status(&pool, old_local).await?, "running");
        assert_eq!(status(&pool, expired_local).await?, "terminating");
        assert_eq!(status(&pool, live_local).await?, "running");
        let ttl_reason: String =
            sqlx::query_scalar("SELECT shutdown_reason FROM sandboxes WHERE id=$1")
                .bind(expired_local)
                .fetch_one(&pool)
                .await?;
        assert_eq!(ttl_reason, "sandbox_ttl_expired");
        let reason: String =
            sqlx::query_scalar("SELECT shutdown_reason FROM sandboxes WHERE id=$1")
                .bind(old_cloud)
                .fetch_one(&pool)
                .await?;
        assert_eq!(reason, "cloud_lifetime_limit");
        schedule_expiry(&pool).await?;
        let jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sandbox_lifecycle_jobs WHERE sandbox_id=$1")
                .bind(old_cloud)
                .fetch_one(&pool)
                .await?;
        assert_eq!(jobs, 1);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "requires isolated DATABASE_URL and MAGICMTN_TEST_MIGRATIONS"]
    async fn durable_intent_and_crash_after_removal(pool: PgPool) -> Result<()> {
        migrate(&pool).await?;
        let id = sandbox(&pool).await?;
        let (s, t) = session(&pool, id).await?;
        // Rollback must leave both the resource and work untouched.
        let mut tx = pool.begin().await?;
        request_termination(&mut tx, id, "sandbox_terminated").await?;
        tx.rollback().await?;
        assert_eq!(status(&pool, id).await?, "running");
        assert!(claim(&pool, Some(id)).await?.is_none());
        request(&pool, id).await?;
        assert_eq!(status(&pool, id).await?, "terminating");
        let turn: (String, Option<Uuid>) =
            sqlx::query_as("SELECT state,lease_token FROM chat_turns WHERE id=$1")
                .bind(t)
                .fetch_one(&pool)
                .await?;
        assert_eq!(turn, ("stopped".into(), None));
        let session_state: String =
            sqlx::query_scalar("SELECT state FROM chat_sessions WHERE id=$1")
                .bind(s)
                .fetch_one(&pool)
                .await?;
        assert_eq!(session_state, "ended");
        let job = claim(&pool, Some(id)).await?.unwrap();
        request(&pool, id).await?; // Repeated requests cannot steal the lease.
        assert!(claim(&pool, Some(id)).await?.is_none());
        let fake = Fake::new(SandboxState::Running);
        execute(&pool.clone().into(), &fake, &job).await?; // Simulate crash before finish commits.
        assert_eq!(fake.removals.load(Ordering::SeqCst), 1);
        sqlx::query("UPDATE sandbox_lifecycle_jobs SET lease_expires_at=now()-interval '1 second' WHERE sandbox_id=$1").bind(id).execute(&pool).await?;
        run_one(&pool, &fake, Some(id)).await?;
        assert_eq!(status(&pool, id).await?, "ended");
        assert_eq!(fake.removals.load(Ordering::SeqCst), 1); // Missing needs no second delete.
        assert!(!finish(&pool, &job, &Ok(Observation::Terminated)).await?);
        request(&pool, id).await?;
        assert!(!run_one(&pool, &fake, Some(id)).await?);
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "requires isolated DATABASE_URL and MAGICMTN_TEST_MIGRATIONS"]
    async fn outage_and_unsupported_inspection_preserve_status(pool: PgPool) -> Result<()> {
        migrate(&pool).await?;
        let id = sandbox(&pool).await?;
        schedule(&pool).await?;
        let mut fake = Fake::new(SandboxState::Missing);
        fake.unavailable = true;
        run_one(&pool, &fake, Some(id)).await?;
        assert_eq!(status(&pool, id).await?, "running");
        let checked: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT runtime_checked_at FROM sandboxes WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        assert!(checked.is_none());
        let retry: (bool,String)=sqlx::query_as("SELECT next_attempt_at>now(),last_error FROM sandbox_lifecycle_jobs WHERE sandbox_id=$1").bind(id).fetch_one(&pool).await?;
        assert!(retry.0);
        assert_eq!(retry.1, "runtime unavailable");
        // Promotion must immediately wake a backed-off inspection.
        request(&pool, id).await?;
        run_one(&pool, &fake, Some(id)).await?;
        assert_eq!(status(&pool, id).await?, "terminating");
        let id2 = sandbox(&pool).await?;
        schedule(&pool).await?;
        fake.unavailable = false;
        fake.supports_status = false;
        run_one(&pool, &fake, Some(id2)).await?;
        assert_eq!(status(&pool, id2).await?, "running");
        let error: String = sqlx::query_scalar("SELECT runtime_error FROM sandboxes WHERE id=$1")
            .bind(id2)
            .fetch_one(&pool)
            .await?;
        assert_eq!(error, "provider_status_unsupported");
        request(&pool, id2).await?;
        run_one(&pool, &fake, Some(id2)).await?;
        assert_eq!(status(&pool, id2).await?, "ended");
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "requires isolated DATABASE_URL and MAGICMTN_TEST_MIGRATIONS"]
    async fn reconciliation_fences_work_and_late_inspection(pool: PgPool) -> Result<()> {
        migrate(&pool).await?;
        let id = sandbox(&pool).await?;
        let (s, t) = session(&pool, id).await?;
        schedule(&pool).await?;
        let job = claim(&pool, Some(id)).await?.unwrap();
        // Cancellation supersedes an observation already in flight.
        request(&pool, id).await?;
        assert!(!finish(&pool, &job, &Ok(Observation::State(SandboxState::Running))).await?);
        assert_eq!(status(&pool, id).await?, "terminating");
        run_one(&pool, &Fake::new(SandboxState::Missing), Some(id)).await?;
        assert_eq!(status(&pool, id).await?, "ended");
        let id2 = sandbox(&pool).await?;
        let (s2, t2) = session(&pool, id2).await?;
        schedule(&pool).await?;
        run_one(&pool, &Fake::new(SandboxState::Missing), Some(id2)).await?;
        assert_eq!(status(&pool, id2).await?, "ended");
        let states: (String,String,Option<Uuid>)=sqlx::query_as("SELECT s.state,t.state,t.lease_token FROM chat_sessions s JOIN chat_turns t ON t.session_id=s.id WHERE s.id=$1 AND t.id=$2").bind(s2).bind(t2).fetch_one(&pool).await?;
        assert_eq!(states, ("interrupted".into(), "stopped".into(), None));
        let reason: String =
            sqlx::query_scalar("SELECT shutdown_reason FROM sandboxes WHERE id=$1")
                .bind(id2)
                .fetch_one(&pool)
                .await?;
        assert_eq!(reason, "runtime_missing");
        // Check the cancelled session remains ended, not interrupted.
        let states: (String,String)=sqlx::query_as("SELECT s.state,t.state FROM chat_sessions s JOIN chat_turns t ON t.session_id=s.id WHERE s.id=$1 AND t.id=$2").bind(s).bind(t).fetch_one(&pool).await?;
        assert_eq!(states, ("ended".into(), "stopped".into()));
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    #[ignore = "requires isolated DATABASE_URL and MAGICMTN_TEST_MIGRATIONS"]
    async fn concurrent_claims_and_legacy_recovery(pool: PgPool) -> Result<()> {
        migrate(&pool).await?;
        let id = sandbox(&pool).await?;
        let (s, _) = session(&pool, id).await?;
        sqlx::query("UPDATE chat_sessions SET state='ended' WHERE id=$1")
            .bind(s)
            .execute(&pool)
            .await?;
        schedule(&pool).await?;
        assert_eq!(status(&pool, id).await?, "terminating");
        let (a, b) = tokio::try_join!(claim(&pool, Some(id)), claim(&pool, Some(id)))?;
        assert_ne!(a.is_some(), b.is_some());
        let job = a.or(b).unwrap();
        assert_eq!(job.operation, "terminate");
        sqlx::query("UPDATE sandbox_lifecycle_jobs SET lease_expires_at=now()-interval '1 second' WHERE sandbox_id=$1").bind(id).execute(&pool).await?;
        assert!(!finish(&pool, &job, &Ok(Observation::Terminated)).await?);
        let replacement = claim(&pool, Some(id)).await?.unwrap();
        assert_ne!(job.token, replacement.token);
        assert!(!finish(&pool, &job, &Ok(Observation::Terminated)).await?);
        finish(&pool, &replacement, &Ok(Observation::Terminated)).await?;
        let id2 = sandbox(&pool).await?;
        let (s2, _) = session(&pool, id2).await?;
        schedule(&pool).await?;
        run_one(&pool, &Fake::new(SandboxState::Stopped), Some(id2)).await?;
        assert_eq!(status(&pool, id2).await?, "stopped");
        let session_state: String =
            sqlx::query_scalar("SELECT state FROM chat_sessions WHERE id=$1")
                .bind(s2)
                .fetch_one(&pool)
                .await?;
        assert_eq!(session_state, "interrupted");
        let id3 = sandbox(&pool).await?;
        sqlx::query("UPDATE sandboxes SET status='terminating' WHERE id=$1")
            .bind(id3)
            .execute(&pool)
            .await?;
        schedule(&pool).await?;
        run_one(&pool, &Fake::new(SandboxState::Missing), Some(id3)).await?;
        assert_eq!(status(&pool, id3).await?, "ended");
        Ok(())
    }
}
