//! Shared sandbox API. The host authenticates requests and supplies resource scope and policy.
use crate::workspace::State;
use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use magicmtn_provider_framework::host::Registry;
use magicmtn_sdk::plugin::{Operation, Value as ProviderValue};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;

use uuid::Uuid;
mod workloads;
use crate::sandbox_storage::Store;

#[derive(Clone)]
pub struct ServiceState {
    pub store: Store,
    pub registry: Registry,
}
impl From<SandboxState> for ServiceState {
    fn from(state: SandboxState) -> Self {
        Self {
            store: state.pool.into(),
            registry: state.registry,
        }
    }
}
impl crate::workspace::WorkspaceState for ServiceState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.store = pool.into();
        self
    }
}

#[derive(Clone)]
pub struct SandboxState {
    pub pool: PgPool,
    pub registry: Registry,
}
pub type ApiError = (StatusCode, String);
type Reply = Result<Json<Value>, ApiError>;
pub type PolicyFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ApiError>> + Send + 'a>>;

/// Host policy around shared durable creation. Cloud owns billing and identity;
/// the local assembly supplies one resource scope and no spending admission.
pub trait SandboxPolicy: Send + Sync {
    /// SQLite hosts explicitly opt into admission without Cloud policy hooks.
    fn admit_local(&self) -> Result<(), ApiError> {
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "policy requires PostgreSQL".into(),
        ))
    }
    fn ttl_seconds(&self, _provider: &str) -> Result<u32, ApiError>;
    fn default_template(&self) -> Option<String> {
        None
    }
    fn preferred_connection(&self, _provider: &str) -> Option<String> {
        None
    }
    fn user_id(&self) -> Option<Uuid> {
        None
    }
    fn allow_provider(&self, _provider: &str) -> Result<(), ApiError> {
        Ok(())
    }
    fn public_record(&self, record: Value) -> Value {
        record
    }
    fn admit<'a>(
        &'a self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _user: Option<Uuid>,
    ) -> PolicyFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn authorize<'a>(
        &'a self,
        _pool: &'a PgPool,
        _user: Option<Uuid>,
        _binding: &'a crate::provider_connections::Binding,
        _id: Uuid,
        _ttl_seconds: u32,
        _specification: &'a str,
    ) -> PolicyFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

pub struct LocalPolicy {
    pub ttl_seconds: u32,
}
impl SandboxPolicy for LocalPolicy {
    fn default_template(&self) -> Option<String> {
        let platform = magicmtn_sdk::image_registry::local_platform().ok()?;
        let image = magicmtn_sdk::image_registry::canonical(platform).ok()?;
        Some(format!("image:{}", image.reference()))
    }
    fn admit_local(&self) -> Result<(), ApiError> {
        Ok(())
    }
    fn preferred_connection(&self, provider: &str) -> Option<String> {
        Some(format!("connection:self-{provider}"))
    }
    fn ttl_seconds(&self, _provider: &str) -> Result<u32, ApiError> {
        if !(1..=86400).contains(&self.ttl_seconds) {
            return Err(invalid(
                "sandbox lifetime must be between 1 and 86400 seconds",
            ));
        }
        Ok(self.ttl_seconds)
    }
}

fn failed(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(%error, "sandbox operation failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "sandbox operation failed".into(),
    )
}
fn invalid(message: &str) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}
fn image_specification(selection: &str, id: Uuid) -> Result<String, (StatusCode, String)> {
    let image = selection
        .strip_prefix("image:")
        .ok_or_else(|| invalid("sandbox creation requires an image reference"))?;
    if image.is_empty()
        || image.len() > 512
        || image.starts_with('-')
        || !image
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_.:@-".contains(&c))
    {
        return Err(invalid("invalid OCI image reference"));
    }
    Ok(format!("image:{image}|{id}"))
}
pub fn router(state: impl Into<ServiceState>, policy: std::sync::Arc<dyn SandboxPolicy>) -> Router {
    Router::new()
        .route("/v1/sandboxes", get(list).post(create))
        .route(
            "/v1/sandboxes/:id/exec",
            post(workloads::execute).layer(axum::extract::DefaultBodyLimit::max(192 * 1024)),
        )
        .route("/v1/sandboxes/:id", get(show))
        .route("/v1/sandboxes/:id/terminate", post(terminate))
        .layer(axum::Extension(policy))
        .with_state(state.into())
}
async fn list(
    State(s): State<ServiceState>,
    axum::Extension(policy): axum::Extension<std::sync::Arc<dyn SandboxPolicy>>,
) -> Reply {
    let records = s.store.records(None).await.map_err(failed)?;
    Ok(Json(json!(records
        .into_iter()
        .map(|record| policy.public_record(record))
        .collect::<Vec<_>>())))
}
fn valid_prefix(id: &str) -> bool {
    !id.is_empty() && id.len() <= 36 && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}
pub async fn resolve(pool: &PgPool, id: &str) -> Result<Value, (StatusCode, String)> {
    resolve_store(&pool.clone().into(), id).await
}

pub async fn resolve_store(store: &Store, id: &str) -> Result<Value, ApiError> {
    if !valid_prefix(id) {
        return Err(invalid("expected a sandbox UUID or hexadecimal prefix"));
    }
    let records = store.records(Some(id)).await.map_err(failed)?;
    match records.as_slice() {
        [record] => Ok(record.clone()),
        [] => Err((StatusCode::NOT_FOUND, "sandbox not found".into())),
        _ => Err((
            StatusCode::CONFLICT,
            "sandbox prefix is ambiguous; use the full UUID".into(),
        )),
    }
}
async fn show(
    State(s): State<ServiceState>,
    Path(id): Path<String>,
    axum::Extension(policy): axum::Extension<std::sync::Arc<dyn SandboxPolicy>>,
) -> Reply {
    resolve_store(&s.store, &id)
        .await
        .map(|record| Json(policy.public_record(record)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    #[serde(default)]
    pub template: String,
    pub provider: String,
    pub connection_id: Option<Uuid>,
    pub idempotency_key: Option<Uuid>,
    pub notes: Option<String>,
}
async fn create(
    State(s): State<ServiceState>,
    axum::Extension(policy): axum::Extension<std::sync::Arc<dyn SandboxPolicy>>,
    Json(body): Json<Create>,
) -> Reply {
    policy.allow_provider(&body.provider)?;
    create_record(s, body, None, policy.as_ref())
        .await
        .map(|record| Json(policy.public_record(record)))
}

// Pi workspaces share the same durable create receipts and lifecycle as every
// other sandbox. Their immutable launch selections are saved before dispatch.
pub async fn create_record(
    s: impl Into<ServiceState>,
    mut body: Create,
    workspace: Option<Value>,
    policy: &dyn SandboxPolicy,
) -> Result<Value, (StatusCode, String)> {
    let s = s.into();
    let id = body.idempotency_key.unwrap_or_else(Uuid::new_v4);
    if body.template.is_empty() {
        if let Some(default) = policy.default_template() {
            // A retry retains its original image across upgrades to the default.
            body.template = match s.store.get(id).await.map_err(failed)? {
                Some(saved) => saved["template"].as_str().unwrap_or("").to_owned(),
                None => default,
            };
        }
    }
    let specification = image_specification(&body.template, id)?;
    let snapshot_key = match workspace.as_ref().and_then(|w| w["kind"].as_str()) {
        Some("workload") => "_magicmtn_workload",
        Some("general-agent") => "_magicmtn_general_agent",
        _ => "_magicmtn_workspace",
    };
    let binding = s
        .store
        .binding(
            body.connection_id,
            policy.preferred_connection(&body.provider).as_deref(),
            &body.provider,
        )
        .await
        .map_err(|error| {
            if error.is::<crate::sandbox_storage::ConnectionSelectionRequired>() {
                invalid("select an explicit provider connection")
            } else {
                failed(error)
            }
        })?;
    if binding.pin.name != body.provider {
        return Err(invalid("provider differs from connection"));
    }
    crate::provider_connections::ensure_sandbox(&binding).map_err(failed)?;
    let pin = binding.pin.clone();
    if body
        .notes
        .as_ref()
        .is_some_and(|notes| notes.chars().count() > 30)
    {
        return Err(invalid("notes must be at most 30 characters"));
    }
    if let Some(saved) = s.store.get(id).await.map_err(failed)? {
        if saved["template"] != body.template
            || saved["metadata"]["_magicmtn_connection"]["connection_id"]
                != json!(binding.connection_id)
            || saved["metadata"].get(snapshot_key) != workspace.as_ref()
        {
            return Err(invalid("idempotency key conflicts"));
        }
        return Ok(saved);
    }
    let scope = format!("sandbox:{id}");
    let prepared =
        crate::provider_connections::admit_credential(&s.registry, &binding).map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "provider credential unavailable".into(),
            )
        })?;
    let mut metadata =
        json!({"_magicmtn_connection":binding,"_magicmtn_provider":pin,"_magicmtn_scope":scope});
    if let Some(workspace) = &workspace {
        metadata[snapshot_key] = workspace.clone();
    }
    let user = policy.user_id();
    let ttl_seconds = policy.ttl_seconds(&binding.pin.name)?;
    s.store
        .reserve(
            id,
            &body,
            &metadata,
            snapshot_key,
            user,
            ttl_seconds,
            policy,
        )
        .await?;
    match &s.store {
        Store::Postgres(pool) => {
            if let Err(error) = policy
                .authorize(pool, user, &binding, id, ttl_seconds, &specification)
                .await
            {
                s.store
                    .failure(id, "failed", "spending authorization failed")
                    .await
                    .map_err(failed)?;
                return Err(error);
            }
        }
        #[cfg(feature = "sqlite")]
        Store::Sqlite(_) => {}
    }
    let result = crate::provider_connections::dispatch_prepared(
        &s.store,
        &s.registry,
        &binding,
        &scope,
        Operation::SandboxCreate {
            template: specification,
            ttl_seconds: Some(ttl_seconds),
        },
        Duration::from_secs(120),
        false,
        prepared,
    )
    .await;
    let mut sandbox = match result {
        Ok(ProviderValue::Sandbox(sandbox)) => sandbox,
        Err(error) if error.is::<crate::provider_connections::CredentialUnavailable>() => {
            s.store
                .failure(id, "failed", "provider credential unavailable")
                .await
                .map_err(failed)?;
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "provider credential unavailable".into(),
            ));
        }
        other => {
            s.store
                .failure(id, "uncertain", "create outcome uncertain; do not replay")
                .await
                .map_err(failed)?;
            return Err(failed(match other {
                Err(error) => error,
                _ => anyhow::anyhow!("invalid sandbox response"),
            }));
        }
    };
    let mut metadata = sandbox.metadata.take().unwrap_or_else(|| json!({}));
    metadata["_magicmtn_connection"] = json!(binding);
    metadata["provider"] = json!(pin.name);
    metadata["_magicmtn_provider"] = json!(pin);
    metadata["_magicmtn_scope"] = json!(scope);
    if let Some(workspace) = workspace {
        metadata[snapshot_key] = workspace;
    }
    sandbox.metadata = Some(metadata);
    s.store.attach(id, &sandbox, false).await.map_err(failed)?;
    s.store
        .get(id)
        .await
        .map_err(failed)?
        .ok_or_else(|| failed("sandbox missing"))
}
async fn terminate(
    State(s): State<ServiceState>,
    Path(prefix): Path<String>,
    axum::Extension(policy): axum::Extension<std::sync::Arc<dyn SandboxPolicy>>,
) -> Reply {
    terminate_record(s, &prefix)
        .await
        .map(|record| Json(policy.public_record(record)))
}

pub async fn terminate_record(s: impl Into<ServiceState>, prefix: &str) -> Result<Value, ApiError> {
    let s = s.into();
    let record = resolve_store(&s.store, prefix).await?;
    let id: Uuid = serde_json::from_value(record["id"].clone()).map_err(failed)?;
    s.store
        .request_termination(id, "sandbox_terminated")
        .await
        .map_err(failed)?;
    // The intent is already durable. Provider failures leave a visible
    // terminating record and are retried by the independent server worker.
    crate::sandbox_lifecycle::run_one_store(&s.store, &s.registry, Some(id))
        .await
        .map_err(failed)?;
    resolve_store(&s.store, &id.to_string()).await
}

impl crate::workspace::WorkspaceState for SandboxState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}

/// Validate availability consistently for bounded commands and local terminals.
pub fn execution_sandbox(saved: &Value) -> Result<magicmtn_sdk::ProviderSandbox, ApiError> {
    if !matches!(
        saved["status"].as_str(),
        Some("running" | "starting" | "RUNNING" | "ready" | "PENDING" | "STARTING" | "pending")
    ) || saved["provider_id"]
        .as_str()
        .is_none_or(|id| id.starts_with("pending:"))
    {
        return Err((
            StatusCode::CONFLICT,
            "sandbox is not available for execution".into(),
        ));
    }
    if let Some(expires) = saved["expires_at"].as_str() {
        if chrono::DateTime::parse_from_rfc3339(expires).map_err(failed)? <= chrono::Utc::now() {
            return Err((StatusCode::CONFLICT, "sandbox has expired".into()));
        }
    }
    Ok(magicmtn_sdk::ProviderSandbox {
        provider_id: saved["provider_id"].as_str().unwrap().into(),
        status: saved["status"].as_str().map(str::to_owned),
        create_time_ms: saved["create_time_ms"].as_i64(),
        metadata: Some(saved["metadata"].clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_launch_preserves_provider_protocol() {
        let id = Uuid::new_v4();
        for image in [
            "image:localhost/my-agent:dev",
            "image:registry.example/magicmtn-default@sha256:abc",
        ] {
            assert_eq!(
                image_specification(image, id).unwrap(),
                format!("{image}|{id}")
            );
        }
        for invalid_image in [
            "browser_use",
            "/tmp/Containerfile",
            "image:",
            "image:-invalid",
            "image:bad image",
        ] {
            assert_eq!(
                image_specification(invalid_image, id).unwrap_err().0,
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[tokio::test]
    async fn named_templates_are_unavailable_without_accessing_resources() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let directory = tempfile::tempdir().unwrap();
        let app = router(
            SandboxState {
                pool: PgPool::connect_lazy("postgresql://unused@127.0.0.1/unused").unwrap(),
                registry: Registry::new(directory.path().into()).unwrap(),
            },
            std::sync::Arc::new(LocalPolicy { ttl_seconds: 3600 }),
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/sandbox-templates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sandboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"template":"browser_use","provider":"apple-container"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
