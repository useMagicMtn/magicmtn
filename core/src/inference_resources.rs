//! Serving resources outlive provider processes and may serve several sessions.
use crate::{
    provider_connections::{self, Binding},
    workspace::{self, State},
};
use axum::{
    extract::Path,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use magicmtn_provider_framework::host::Registry;
use magicmtn_sdk::{
    plugin::{Operation, Value as ProviderValue},
    InferenceDeploymentRef, InferenceDeploymentSpec,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct ResourceState {
    pub pool: PgPool,
    pub registry: Registry,
}
impl workspace::WorkspaceState for ResourceState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
type Reply = Result<Json<Value>, (StatusCode, String)>;
fn failure(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::debug!(reason=%error,"inference resource operation rejected");
    (
        StatusCode::CONFLICT,
        "inference resource unavailable or operation outcome uncertain".into(),
    )
}
pub fn router(state: ResourceState) -> Router {
    Router::new()
        .route("/v1/inference-resources", get(list).post(create))
        .route("/v1/inference-resources/:id/:operation", post(change))
        .with_state(state)
}
async fn list(State(s): State<ResourceState>) -> Reply {
    let records: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM inference_resources r ORDER BY created_at,id")
            .fetch_all(&s.pool)
            .await
            .map_err(failure)?;
    Ok(Json(
        json!({"resources":records.into_iter().filter(|r| r["binding"]["pin"]["name"].as_str().is_some_and(crate::public_providers::contains)).collect::<Vec<_>>()}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    connection_id: Uuid,
    specification: InferenceDeploymentSpec,
    idempotency_key: Uuid,
}
async fn create(State(s): State<ResourceState>, Json(mut body): Json<Create>) -> Reply {
    let binding = provider_connections::by_id(&s.pool, body.connection_id)
        .await
        .map_err(failure)?;
    crate::public_providers::require(&binding.pin.name)?;
    // Provider names are globally distinct within a shared managed account.
    body.specification.name = format!("mm-{}", body.idempotency_key.simple());
    let scope = format!("inference:{}", body.idempotency_key);
    let store: crate::sandbox_storage::Store = s.pool.clone().into();
    let prepared = match provider_connections::admit_credential(&s.registry, &binding) {
        Ok(prepared) => prepared,
        Err(error) => {
            let saved = sqlx::query_scalar("SELECT to_jsonb(r) FROM inference_resources r WHERE id=$1 AND connection_id=$2 AND specification=$3")
                .bind(body.idempotency_key).bind(binding.connection_id).bind(json!(body.specification))
                .fetch_optional(&s.pool).await.map_err(failure)?;
            return saved.map(Json).ok_or_else(|| failure(error));
        }
    };
    let row=sqlx::query("INSERT INTO inference_resources(id,organization_id,connection_id,binding,resource_name,specification) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING RETURNING id")
        .bind(body.idempotency_key).bind(binding.organization_id).bind(binding.connection_id).bind(json!(binding))
        .bind(&body.specification.name).bind(json!(body.specification)).fetch_optional(&s.pool).await.map_err(failure)?;
    if row.is_none() {
        let saved:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM inference_resources r WHERE id=$1 AND connection_id=$2 AND specification=$3")
            .bind(body.idempotency_key).bind(binding.connection_id).bind(json!(body.specification)).fetch_one(&s.pool).await.map_err(failure)?;
        return Ok(Json(saved));
    }
    let result = provider_connections::dispatch_prepared(
        &store,
        &s.registry,
        &binding,
        &scope,
        Operation::InferenceDeploymentCreate {
            spec: body.specification,
        },
        Duration::from_secs(120),
        false,
        prepared,
    )
    .await;
    save(&s, body.idempotency_key, result).await
}
async fn change(
    State(s): State<ResourceState>,
    Path((id, operation)): Path<(Uuid, String)>,
) -> Reply {
    let row = sqlx::query("SELECT binding,provider_id,state FROM inference_resources WHERE id=$1")
        .bind(id)
        .fetch_one(&s.pool)
        .await
        .map_err(failure)?;
    let binding: Binding = serde_json::from_value(row.get("binding")).map_err(failure)?;
    crate::public_providers::require(&binding.pin.name)?;
    let resource = InferenceDeploymentRef {
        provider_id: row
            .get::<Option<String>, _>("provider_id")
            .ok_or_else(|| failure("create must be reconciled"))?,
    };
    let op = match operation.as_str() {
        "status" => Operation::InferenceDeploymentStatus { resource },
        "start" => Operation::InferenceDeploymentStart { resource },
        "stop" => Operation::InferenceDeploymentStop { resource },
        "delete" => Operation::InferenceDeploymentDelete { resource },
        _ => return Err(failure("unknown transition")),
    };
    let prepared =
        provider_connections::admit_credential(&s.registry, &binding).map_err(failure)?;
    if operation != "status" {
        let allowed = match operation.as_str() {
            "start" => vec!["stopped"],
            "stop" => vec!["active"],
            "delete" => vec!["active", "stopped", "failed"],
            _ => vec![],
        };
        let changed =
            sqlx::query("UPDATE inference_resources SET state=$2 WHERE id=$1 AND state=ANY($3)")
                .bind(id)
                .bind(if operation == "delete" {
                    "deleting"
                } else {
                    "uncertain"
                })
                .bind(allowed)
                .execute(&s.pool)
                .await
                .map_err(failure)?;
        if changed.rows_affected() != 1 {
            return Err(failure(
                "transition already attempted; inspect rather than replay",
            ));
        }
    }
    let result = provider_connections::dispatch_prepared(
        &s.pool.clone().into(),
        &s.registry,
        &binding,
        &format!("inference:{id}"),
        op,
        Duration::from_secs(120),
        operation == "delete" || operation == "stop" || operation == "status",
        prepared,
    )
    .await;
    save(&s, id, result).await
}
async fn save(s: &ResourceState, id: Uuid, result: anyhow::Result<ProviderValue>) -> Reply {
    match result {
        Ok(ProviderValue::InferenceDeployment(snapshot)) => {
            use magicmtn_sdk::InferenceDeploymentState::*;
            let state = match snapshot.state {
                Stopped => "stopped",
                Deleted => "deleted",
                Deleting => "deleting",
                Failed => "failed",
                Unknown => "uncertain",
                _ => "active",
            };
            let changed=sqlx::query("UPDATE inference_resources SET provider_id=$2,snapshot=$3,state=$4,last_error=NULL WHERE id=$1 AND (provider_id IS NULL OR provider_id=$2)")
                .bind(id).bind(&snapshot.resource.provider_id).bind(json!(snapshot)).bind(state).execute(&s.pool).await.map_err(failure)?;
            if changed.rows_affected() != 1 {
                return Err(failure("provider returned another resource identity"));
            }
        }
        Ok(ProviderValue::Done) => {} // Deletion acknowledgement is not deletion confirmation.
        Err(error) => {
            sqlx::query("UPDATE inference_resources SET state='uncertain',last_error='provider operation outcome uncertain; inspect before retry' WHERE id=$1")
                .bind(id).execute(&s.pool).await.map_err(failure)?;
            return Err(failure(error));
        }
        _ => return Err(failure("invalid lifecycle response")),
    }
    let record: Value =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM inference_resources r WHERE id=$1")
            .bind(id)
            .fetch_one(&s.pool)
            .await
            .map_err(failure)?;
    Ok(Json(record))
}
