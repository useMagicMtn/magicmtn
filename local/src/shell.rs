//! Local terminal handoff; the CLI owns the controlling terminal.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use magicmtn_engine::{provider_connections::Binding, sandbox_storage::Store, sandboxes};
use magicmtn_provider_framework::host::Registry;
use serde_json::{json, Value};

pub(super) fn router(store: Store, registry: Registry) -> Router {
    Router::new()
        .route("/v1/sandboxes/:id/shell", post(connection))
        .with_state((store, registry))
}

async fn connection(
    State((store, registry)): State<(Store, Registry)>,
    Path(id): Path<String>,
) -> Result<Json<Value>, sandboxes::ApiError> {
    let saved = sandboxes::resolve_store(&store, &id).await?;
    let sandbox = sandboxes::execution_sandbox(&saved)?;
    let invalid = || {
        (
            StatusCode::CONFLICT,
            "local shell connection unavailable".into(),
        )
    };
    let binding: Binding =
        serde_json::from_value(saved["metadata"]["_magicmtn_connection"].clone())
            .map_err(|_| invalid())?;
    store
        .validate_binding(&binding, false)
        .await
        .map_err(|_| invalid())?;
    let installed = registry
        .installed_pin(&binding.pin)
        .map_err(|_| invalid())?;
    if binding.runner_id.is_some()
        || installed.manifest.credential.is_some()
        || !installed
            .manifest
            .capabilities
            .sandbox_operations
            .contains(&magicmtn_sdk::plugin::SandboxOperation::Ssh)
    {
        return Err(invalid());
    }
    Ok(Json(
        json!({"pin":binding.pin,"configuration":binding.configuration,"sandbox":sandbox}),
    ))
}
