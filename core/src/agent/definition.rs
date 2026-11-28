//! Atomic revision authoring. Dashboard actors and retry receipts include the
//! authenticated organization/account context; machine APIs retain service scopes.
use crate::{auth::Principal, contracts};
use axum::http::StatusCode;
use magicmtn_contracts::{artifacts, configuration::*};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
pub type Failure = (StatusCode, &'static str);
pub type Result<T> = std::result::Result<T, Failure>;
fn database(e: sqlx::Error) -> Failure {
    tracing::error!(code = ?e.as_database_error().and_then(|e| e.code()), "configuration transaction failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not save this agent.",
    )
}
fn invalid(_: impl std::fmt::Display) -> Failure {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "Invalid prompt, model or compute selection, or runtime configuration.",
    )
}
fn conflict() -> Failure {
    (
        StatusCode::CONFLICT,
        "This agent changed. Reload it before saving again.",
    )
}
fn denied() -> Failure {
    (
        StatusCode::FORBIDDEN,
        "Configuration action or model use denied.",
    )
}
#[derive(Clone, Copy)]
pub enum Actor<'a> {
    Local,
    Service(&'a Principal),
}
impl Actor<'_> {
    fn reference(self) -> String {
        match self {
            Self::Local => crate::registry_scope::actor_reference("local-dashboard"),
            Self::Service(p) => p.principal_ref.clone(),
        }
    }
    fn require(self, action: &str, h: Uuid) -> Result<()> {
        match self {
            Self::Local => Ok(()),
            Self::Service(p) => p.require(action, h).map_err(|_| denied()),
        }
    }
    fn create(self) -> Result<()> {
        match self {
            Self::Local => Ok(()),
            Self::Service(p) if p.actions.iter().any(|a| a == "harness_create") => Ok(()),
            _ => Err(denied()),
        }
    }
    fn compute(self, id: Uuid) -> Result<()> {
        match self {
            Self::Local => Ok(()),
            Self::Service(p) => p.require_compute(id).map_err(|_| denied()),
        }
    }
    fn model(self, id: Uuid) -> Result<()> {
        match self {
            Self::Local => Ok(()),
            Self::Service(p) => p.require_model(id).map_err(|_| denied()),
        }
    }
}
fn key(k: &str) -> Result<()> {
    if k.is_empty() || k.len() > 200 {
        return Err(invalid("key"));
    }
    Ok(())
}
fn ids(ids: &mut [Uuid], default: Uuid) -> Result<()> {
    ids.sort_unstable();
    if ids.is_empty()
        || ids.iter().any(Uuid::is_nil)
        || ids.windows(2).any(|a| a[0] == a[1])
        || !ids.contains(&default)
    {
        return Err(invalid("models"));
    }
    Ok(())
}
fn metadata(name: &str, description: Option<&str>) -> Result<()> {
    if name.trim().is_empty()
        || name.chars().count() > 120
        || description.is_some_and(|d| d.chars().count() > 2000)
    {
        return Err(invalid("metadata"));
    }
    Ok(())
}
async fn begin(
    pool: &PgPool,
    actor: Actor<'_>,
    kind: &str,
    resource: &str,
    key: &str,
) -> Result<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await.map_err(database)?;
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    sqlx::query("SET LOCAL statement_timeout='15s'")
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    // Reserve even create/no-op keys before deciding identities or resolving state.
    let lock = json!([actor.reference(), kind, resource, key]).to_string();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(lock)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    Ok(tx)
}
async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    kind: &str,
    resource: &str,
    key: &str,
    digest: &str,
) -> Result<Option<Value>> {
    let row=sqlx::query("SELECT request_digest,response FROM harness_configuration_operations WHERE principal_ref=$1 AND operation_kind=$2 AND resource_ref=$3 AND idempotency_key=$4")
 .bind(actor.reference()).bind(kind).bind(resource).bind(key).fetch_optional(&mut **tx).await.map_err(database)?;
    match row {
        Some(r) if r.get::<String, _>("request_digest") == digest => Ok(Some(r.get("response"))),
        Some(_) => Err(conflict()),
        None => Ok(None),
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "Persist the complete revision receipt in the caller's transaction"
)]
async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    kind: &str,
    resource: &str,
    key: &str,
    digest: &str,
    h: Uuid,
    response: &Value,
    surfaces: Vec<&str>,
) -> Result<()> {
    sqlx::query("INSERT INTO harness_configuration_operations(id,principal_ref,operation_kind,resource_ref,idempotency_key,request_digest,harness_id,response,changed_surfaces) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
 .bind(Uuid::new_v4()).bind(actor.reference()).bind(kind).bind(resource).bind(key).bind(digest).bind(h).bind(response).bind(surfaces).execute(&mut **tx).await.map_err(database)?;
    Ok(())
}
async fn lock_head(
    tx: &mut Transaction<'_, Postgres>,
    h: Uuid,
) -> Result<(Option<Uuid>, Option<Uuid>, Option<Uuid>)> {
    let row=sqlx::query("SELECT definition_revision_id,interactive_deployment_id FROM harnesses WHERE id=$1 FOR UPDATE").bind(h).fetch_optional(&mut **tx).await.map_err(database)?.ok_or((StatusCode::NOT_FOUND,"Agent not found."))?;
    let d: Option<Uuid> = row.get("interactive_deployment_id");
    let active = if let Some(d) = d {
        Some(sqlx::query_scalar("SELECT revision_id FROM harness_deployments WHERE id=$1 AND harness_id=$2 FOR UPDATE").bind(d).bind(h).fetch_one(&mut **tx).await.map_err(database)?)
    } else {
        None
    };
    Ok((row.get("definition_revision_id"), d, active))
}
async fn authorize_members(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    h: Uuid,
    revision: Option<Uuid>,
) -> Result<()> {
    if let Some(r) = revision {
        let member_ids: Option<Value> = sqlx::query_scalar(
            "SELECT model_manifest FROM harness_revisions WHERE id=$1 AND harness_id=$2",
        )
        .bind(r)
        .bind(h)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database)?
        .ok_or_else(conflict)?;
        if let Some(entries) = member_ids.and_then(|v| v.as_array().cloned()) {
            for e in entries {
                actor.model(serde_json::from_value(e["model_id"].clone()).map_err(invalid)?)?;
            }
        }
    }
    if let Some(r) = revision {
        let manifest: Option<Value> = sqlx::query_scalar(
            "SELECT compute_manifest FROM harness_revisions WHERE id=$1 AND harness_id=$2",
        )
        .bind(r)
        .bind(h)
        .fetch_one(&mut **tx)
        .await
        .map_err(database)?;
        if let Some(m) = manifest {
            for e in m.as_array().ok_or_else(|| invalid("compute"))? {
                actor.compute(serde_json::from_value(e["compute_id"].clone()).map_err(invalid)?)?;
            }
        }
    }
    Ok(())
}
async fn artifact(tx: &mut Transaction<'_, Postgres>, h: Uuid, r: Uuid) -> Result<Value> {
    let a:Value=sqlx::query_scalar("SELECT a.manifest FROM artifact_manifests a JOIN harness_revisions r ON r.id=a.revision_id WHERE r.id=$1 AND r.harness_id=$2").bind(r).bind(h).fetch_optional(&mut **tx).await.map_err(database)?.ok_or_else(conflict)?;
    contracts::validate_artifact(&a).map_err(invalid)?;
    Ok(a)
}
async fn snapshots(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    selection: &ModelSelection,
    old: &[Value],
) -> Result<Value> {
    let mut entries = Vec::new();
    for id in &selection.model_ids {
        actor.model(*id)?;
        let row =
            sqlx::query("SELECT * FROM tenant_models WHERE id=$1 AND status='enabled' FOR SHARE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(database)?
                .ok_or_else(|| invalid("disabled/unknown registration"))?;
        let e = if !selection.refresh_snapshots {
            old.iter()
                .find(|e| e["model_id"] == id.to_string())
                .cloned()
        } else {
            None
        };
        entries.push(e.unwrap_or_else(||json!({"model_id":id,"snapshot":{"source":row.get::<String,_>("source"),"source_model_id":row.get::<String,_>("source_model_id"),"display_name":row.get::<String,_>("display_name"),"description":row.get::<String,_>("description"),"task_type":row.get::<String,_>("task_type"),"model_configuration":row.get::<Value,_>("model_configuration"),"connection_ref":row.get::<Option<String>,_>("connection_ref"),"capabilities":row.get::<Value,_>("capabilities"),"provenance":"registered"}})));
    }
    Ok(json!(entries))
}
async fn compute_snapshots(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    s: &ComputeSelection,
    old: &[Value],
) -> Result<Value> {
    let mut entries = vec![];
    for id in &s.compute_ids {
        actor.compute(*id)?;
        let row = sqlx::query("SELECT * FROM tenant_compute WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(database)?
            .ok_or_else(|| invalid("unknown/disabled compute"))?;
        let previous = if s.refresh_snapshots {
            None
        } else {
            old.iter()
                .find(|e| e["compute_id"] == id.to_string())
                .cloned()
        };
        if previous.is_none()
            && (row.get::<String, _>("status") != "enabled"
                || row
                    .get::<Option<chrono::DateTime<chrono::Utc>>, _>("retired_at")
                    .is_some())
        {
            return Err(invalid("disabled compute selection or refresh"));
        }
        entries.push(previous.unwrap_or_else(||json!({"compute_id":id,"snapshot":{"display_name":row.get::<String,_>("display_name"),"description":row.get::<String,_>("description"),"backend":row.get::<String,_>("backend"),"binding_ref":row.get::<String,_>("binding_ref"),"configuration":row.get::<Value,_>("configuration"),"capabilities":row.get::<Value,_>("capabilities")}})));
    }
    Ok(json!(entries))
}
fn compute_projection(a: &mut Value) -> Result<()> {
    if a.get("compute_manifest").is_some()
        && a["runtime_version"] == "interactive-v1"
        && a["runtime_configuration"]["state"] == "configured"
    {
        let e = a["compute_manifest"]
            .as_array()
            .ok_or_else(|| invalid("compute manifest"))?
            .iter()
            .find(|e| e["compute_id"] == a["default_compute_id"])
            .ok_or_else(|| invalid("compute default"))?;
        if let Some(image) = e["snapshot"]["configuration"].get("image") {
            a["runtime_configuration"]["sandbox_image"] = image.clone();
        }
    }
    Ok(())
}
async fn prompt(tx: &mut Transaction<'_, Postgres>, h: Uuid, p: &PromptChange) -> Result<Value> {
    if p.system_prompt.trim().is_empty() {
        return Err(invalid("blank prompt"));
    }
    if let Some(id) = p.library_prompt_id {
        let current: String = sqlx::query_scalar(
            "SELECT content FROM system_prompts WHERE id=$1 AND harness_id=$2 FOR UPDATE",
        )
        .bind(id)
        .bind(h)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database)?
        .ok_or((
            StatusCode::NOT_FOUND,
            "Library prompt not found for this harness.",
        ))?;
        if p.expected_library_content_digest.as_deref() != Some(content_digest(&current).as_str()) {
            return Err(conflict());
        }
        if p.update_library {
            sqlx::query("UPDATE system_prompts SET content=$3 WHERE id=$1 AND harness_id=$2")
                .bind(id)
                .bind(h)
                .bind(&p.system_prompt)
                .execute(&mut **tx)
                .await
                .map_err(database)?;
        } else if current != p.system_prompt {
            return Err(conflict());
        }
        Ok(json!({"kind":"library","library_prompt_id":id}))
    } else {
        if p.update_library || p.expected_library_content_digest.is_some() {
            return Err(invalid("inline prompt"));
        }
        Ok(json!({"kind":"inline","library_prompt_id":null}))
    }
}
pub fn content_digest(content: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(content.as_bytes()))
}
fn projection(a: &mut Value) -> Result<()> {
    let e = a["model_manifest"]
        .as_array()
        .ok_or_else(|| invalid("manifest"))?
        .iter()
        .find(|e| e["model_id"] == a["default_model_id"])
        .ok_or_else(|| invalid("default"))?;
    a["model_configuration"] = e["snapshot"]["model_configuration"].clone();
    Ok(())
}
async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    h: Uuid,
    r: Uuid,
    parent: Option<Uuid>,
    a: &Value,
) -> Result<()> {
    let default: Uuid = serde_json::from_value(a["default_model_id"].clone()).map_err(invalid)?;
    let surfaces = json!({"workflow":a["workflow"],"runtime_version":a["runtime_version"],"runtime_configuration":a["runtime_configuration"]});
    sqlx::query("INSERT INTO harness_revisions(id,harness_id,name,model,status,surfaces,tool_manifest,model_configuration,parent_revision_id,model_manifest,default_model_id,system_prompt,prompt_provenance,compute_manifest,default_compute_id) VALUES($1,$2,'Configuration revision',$3,'draft',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
 .bind(r).bind(h).bind(a["model_configuration"]["model"].as_str()).bind(surfaces).bind(&a["tool_manifest"]).bind(&a["model_configuration"]).bind(parent).bind(&a["model_manifest"]).bind(default).bind(a["system_prompt"].as_str()).bind(&a["prompt_provenance"]).bind(a.get("compute_manifest")).bind(a.get("default_compute_id").map(|v|serde_json::from_value::<Uuid>(v.clone())).transpose().map_err(invalid)?).execute(&mut **tx).await.map_err(database)?;
    sqlx::query(
        "INSERT INTO artifact_manifests(revision_id,artifact_digest,manifest) VALUES($1,$2,$3)",
    )
    .bind(r)
    .bind(a["artifact_digest"].as_str())
    .bind(a)
    .execute(&mut **tx)
    .await
    .map_err(database)?;
    Ok(())
}
fn readiness(a: &Value) -> Option<&'static str> {
    if a["runtime_version"] != "interactive-v1"
        || a["runtime_configuration"]["state"] != "configured"
    {
        return Some("Runtime is not configured for interactive execution.");
    }
    if crate::model_runtime::selection(a, None).is_err() {
        return Some(
            "The default model does not support the current interactive provider and tools.",
        );
    }
    None
}
#[expect(
    clippy::too_many_arguments,
    reason = "Keep activation inputs explicit within the caller's transaction"
)]
async fn activate_in(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor<'_>,
    h: Uuid,
    a: &Value,
    mode: Activation,
    expected: Option<Uuid>,
    d: Option<Uuid>,
    active: Option<Uuid>,
    _operation_key: &str,
) -> Result<(Option<Uuid>, Option<Uuid>, Option<&'static str>)> {
    if mode == Activation::Required {
        actor.require("release", h)?;
    }
    if mode == Activation::Draft {
        return Ok((d, active, None));
    }
    let reason = if actor.require("release", h).is_err() {
        Some("Release permission is required.")
    } else {
        readiness(a)
    };
    if let Some(reason) = reason {
        if mode == Activation::Required {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, reason));
        }
        return Ok((d, active, Some(reason)));
    }
    if expected != active {
        return Err(conflict());
    }
    let target: Uuid = serde_json::from_value(a["harness_revision_id"].clone()).map_err(invalid)?;
    for e in a["model_manifest"]
        .as_array()
        .ok_or_else(|| invalid("manifest"))?
    {
        let id: Uuid = serde_json::from_value(e["model_id"].clone()).map_err(invalid)?;
        actor.model(id)?;
        let enabled: bool =
            sqlx::query_scalar("SELECT status='enabled' FROM tenant_models WHERE id=$1 FOR SHARE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(database)?
                .unwrap_or(false);
        if !enabled {
            return Err(invalid("disabled model"));
        }
    }
    if a.get("compute_manifest").is_some() {
        for e in a["compute_manifest"]
            .as_array()
            .ok_or_else(|| invalid("compute"))?
        {
            let id: Uuid = serde_json::from_value(e["compute_id"].clone()).map_err(invalid)?;
            actor.compute(id)?;
            if e["compute_id"] == a["default_compute_id"] {
                let enabled: bool = sqlx::query_scalar(
                    "SELECT status='enabled' AND retired_at IS NULL FROM tenant_compute WHERE id=$1 FOR SHARE",
                )
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(database)?
                .unwrap_or(false);
                if !enabled {
                    return Err(invalid("disabled default compute"));
                }
            }
        }
    }
    if active == Some(target) {
        return Ok((d, active, None));
    }
    sqlx::query("UPDATE harness_revisions SET status='active',activated_at=COALESCE(activated_at,now()) WHERE id=$1 AND harness_id=$2").bind(target).bind(h).execute(&mut **tx).await.map_err(database)?;
    let deployment = if let Some(d) = d {
        sqlx::query("UPDATE harness_deployments SET revision_id=$2 WHERE id=$1 AND harness_id=$3")
            .bind(d)
            .bind(target)
            .bind(h)
            .execute(&mut **tx)
            .await
            .map_err(database)?;
        d
    } else {
        let d = Uuid::new_v4();
        sqlx::query("INSERT INTO harness_deployments(id,harness_id,revision_id,name) VALUES($1,$2,$3,'Interactive assignment')").bind(d).bind(h).bind(target).execute(&mut **tx).await.map_err(database)?;
        d
    };
    sqlx::query("UPDATE harnesses SET interactive_deployment_id=$2 WHERE id=$1")
        .bind(h)
        .bind(deployment)
        .execute(&mut **tx)
        .await
        .map_err(database)?;
    Ok((Some(deployment), Some(target), None))
}
fn response(
    h: Uuid,
    head: Uuid,
    a: &Value,
    d: Option<Uuid>,
    active: Option<Uuid>,
    outcome: &str,
    reason: Option<&str>,
) -> Result<Value> {
    Ok(
        json!({"harness_id":h,"revision_id":a["harness_revision_id"],"definition_revision_id":head,"active_revision_id":active,"deployment_id":d,"configuration_digest":artifacts::configuration_digest(a).map_err(invalid)?,"artifact_digest":a["artifact_digest"],"outcome":outcome,"activation_state":if active==serde_json::from_value(a["harness_revision_id"].clone()).ok(){"active"}else{"draft"},"activation_reason":reason}),
    )
}

pub async fn create(
    pool: &PgPool,
    actor: Actor<'_>,
    mut r: CreateRequest,
    identity: Option<Uuid>,
) -> Result<Value> {
    actor.create()?;
    key(&r.idempotency_key)?;
    metadata(&r.name, r.description.as_deref())?;
    ids(&mut r.model_ids, r.default_model_id)?;
    let default_compute = r
        .default_compute_id
        .ok_or_else(|| invalid("explicit compute selection required"))?;
    ids(&mut r.compute_ids, default_compute)?;
    for id in &r.compute_ids {
        actor.compute(*id)?;
    }
    for id in &r.model_ids {
        actor.model(*id)?;
    }
    if r.expected_active_revision_id.is_some() {
        return Err(conflict());
    }
    let hash = contracts::digest(&json!({"request":r,"identity":identity})).map_err(invalid)?;
    let mut tx = begin(pool, actor, "create", "", &r.idempotency_key).await?;
    if let Some(v) = replay(&mut tx, actor, "create", "", &r.idempotency_key, &hash).await? {
        if r.activation != Activation::Draft && v["activation_state"] == "active" {
            let h: Uuid = serde_json::from_value(v["harness_id"].clone()).map_err(invalid)?;
            actor.require("release", h)?;
        }
        tx.commit().await.map_err(database)?;
        return Ok(v);
    }
    let h = identity.unwrap_or_else(Uuid::new_v4);
    if h.is_nil() {
        return Err(invalid("identity"));
    }
    let inserted=sqlx::query("INSERT INTO harnesses(id,name,workflow,description,created_by,owner_user_id) VALUES($1,$2,$3,$4,$5,usage_user_from_principal($5)) ON CONFLICT DO NOTHING").bind(h).bind(&r.name).bind(&r.workflow).bind(&r.description).bind(actor.reference()).execute(&mut *tx).await.map_err(database)?;
    let (head, d, active) = lock_head(&mut tx, h).await?;
    if head.is_some() || inserted.rows_affected() == 0 {
        return Err(conflict());
    }
    let revision = Uuid::new_v4();
    let selection = ModelSelection {
        model_ids: r.model_ids.clone(),
        default_model_id: r.default_model_id,
        refresh_snapshots: false,
    };
    let manifest = snapshots(&mut tx, actor, &selection, &[]).await?;
    let provenance = if let Some(id) = r.library_prompt_id {
        let p = PromptChange {
            system_prompt: r.system_prompt.clone(),
            library_prompt_id: Some(id),
            expected_library_content_digest: Some(content_digest(&r.system_prompt)),
            update_library: false,
        };
        prompt(&mut tx, h, &p).await?
    } else {
        json!({"kind":"inline","library_prompt_id":null})
    };
    let mut a = json!({"contract_version":1,"canonicalization_version":"ck-json-v1","harness_id":h,"harness_revision_id":revision,"workflow":r.workflow,"runtime_version":r.runtime_version,"system_prompt":r.system_prompt,"prompt_provenance":provenance,"model_manifest":manifest,"default_model_id":r.default_model_id,"model_configuration":{},"tool_manifest":r.tool_manifest,"runtime_configuration":r.runtime_configuration,"artifact_digest":""});
    a["contract_version"] = json!(1);
    a["compute_manifest"] = compute_snapshots(
        &mut tx,
        actor,
        &ComputeSelection {
            compute_ids: r.compute_ids.clone(),
            default_compute_id: default_compute,
            refresh_snapshots: false,
        },
        &[],
    )
    .await?;
    a["default_compute_id"] = json!(default_compute);
    if a["runtime_version"] == "interactive-v1"
        && a["runtime_configuration"]["state"] == "configured"
    {
        let selected = a["compute_manifest"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["compute_id"] == a["default_compute_id"])
            .unwrap();
        if selected["snapshot"]["configuration"].get("image").is_some()
            && a["runtime_configuration"]["sandbox_image"]
                != selected["snapshot"]["configuration"]["image"]
        {
            return Err(invalid("conflicting sandbox image projection"));
        }
    }
    compute_projection(&mut a)?;
    projection(&mut a)?;
    let a = artifacts::seal(&a).map_err(invalid)?;
    insert(&mut tx, h, revision, None, &a).await?;
    sqlx::query("UPDATE harnesses SET definition_revision_id=$2,selected_prompt_id=$3 WHERE id=$1")
        .bind(h)
        .bind(revision)
        .bind(r.library_prompt_id)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
    let (d, active, reason) = activate_in(
        &mut tx,
        actor,
        h,
        &a,
        r.activation,
        None,
        d,
        active,
        &r.idempotency_key,
    )
    .await?;
    let result = response(h, revision, &a, d, active, "created", reason)?;
    receipt(
        &mut tx,
        actor,
        "create",
        "",
        &r.idempotency_key,
        &hash,
        h,
        &result,
        vec!["models", "compute", "default", "prompt", "tools", "runtime"],
    )
    .await?;
    tx.commit().await.map_err(database)?;
    Ok(result)
}
pub async fn save(pool: &PgPool, actor: Actor<'_>, h: Uuid, r: SaveRequest) -> Result<Value> {
    save_inner(pool, actor, h, r, None).await
}
pub async fn save_authoring(
    pool: &PgPool,
    actor: Actor<'_>,
    h: Uuid,
    r: SaveRequest,
    guard: super::authoring::Guard,
) -> Result<Value> {
    save_inner(pool, actor, h, r, Some(guard)).await
}
async fn save_inner(
    pool: &PgPool,
    actor: Actor<'_>,
    h: Uuid,
    mut r: SaveRequest,
    guard: Option<super::authoring::Guard>,
) -> Result<Value> {
    actor.require("harness_edit", h)?;
    key(&r.idempotency_key)?;
    if let Some(s) = &mut r.changes.model_selection {
        ids(&mut s.model_ids, s.default_model_id)?;
        for id in &s.model_ids {
            actor.model(*id)?;
        }
    }
    if let Some(c) = &mut r.changes.compute_selection {
        ids(&mut c.compute_ids, c.default_compute_id)?;
        for id in &c.compute_ids {
            actor.compute(*id)?;
        }
    }
    let hash = if let Some(g) = &guard {
        contracts::digest(&json!({"request":r,"authoring":g.document})).map_err(invalid)?
    } else {
        contracts::digest(&json!(r)).map_err(invalid)?
    };
    let resource = h.to_string();
    let mut tx = begin(pool, actor, "save", &resource, &r.idempotency_key).await?;
    // Replays still require current scope on the original result, before returning evidence.
    if let Some(v) = replay(&mut tx, actor, "save", &resource, &r.idempotency_key, &hash).await? {
        authorize_members(
            &mut tx,
            actor,
            h,
            serde_json::from_value(v["revision_id"].clone()).ok(),
        )
        .await?;
        if r.activation == Activation::Required
            || (r.activation != Activation::Draft && v["activation_state"] == "active")
        {
            actor.require("release", h)?;
        }
        tx.commit().await.map_err(database)?;
        return Ok(v);
    }
    let (head, d, active) = lock_head(&mut tx, h).await?;
    if let Some(g) = &guard {
        let draft = sqlx::query(
            "SELECT generation,document FROM agent_drafts WHERE agent_id=$1 FOR UPDATE",
        )
        .bind(h)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database)?
        .ok_or_else(conflict)?;
        if draft.get::<i64, _>("generation") != g.generation
            || draft.get::<Value, _>("document") != g.document
        {
            return Err(conflict());
        }
    }
    if head != r.expected_base_revision_id {
        return Err(conflict());
    }
    let mut a = if let Some(head) = head {
        artifact(&mut tx, h, head).await?
    } else {
        // Explicit first complete save of an identity-only legacy agent.
        let workflow: String = sqlx::query_scalar("SELECT workflow FROM harnesses WHERE id=$1")
            .bind(h)
            .fetch_one(&mut *tx)
            .await
            .map_err(database)?;
        json!({"contract_version":1,"canonicalization_version":"ck-json-v1","harness_id":h,"harness_revision_id":Uuid::new_v4(),"workflow":workflow,"runtime_version":"unconfigured","runtime_configuration":{"state":"unconfigured"},"tool_manifest":[],"model_manifest":[],"default_model_id":null,"model_configuration":{},"system_prompt":"","prompt_provenance":{"kind":"inline","library_prompt_id":null},"artifact_digest":""})
    };
    let upgraded = a.get("model_manifest").is_none();
    if upgraded {
        if r.changes.model_selection.is_none()
            || r.changes.prompt.is_none()
            || r.changes.runtime.is_none()
        {
            return Err((StatusCode::CONFLICT,"Legacy upgrade requires explicit models, prompt and runtime in the configuration request."));
        }
        let workflow: String = sqlx::query_scalar("SELECT workflow FROM harnesses WHERE id=$1")
            .bind(h)
            .fetch_one(&mut *tx)
            .await
            .map_err(database)?;
        a = json!({"contract_version":1,"canonicalization_version":"ck-json-v1","harness_id":h,"harness_revision_id":Uuid::new_v4(),"workflow":workflow,"runtime_version":"unconfigured","runtime_configuration":{"state":"unconfigured"},"tool_manifest":[],"model_manifest":[],"default_model_id":null,"model_configuration":{},"system_prompt":"","prompt_provenance":{"kind":"inline","library_prompt_id":null},"artifact_digest":""});
    }
    if guard.is_some() {
        a["workflow"] = json!("interactive");
    }
    let old = if head.is_some() && !upgraded {
        Some(artifacts::configuration_digest(&a).map_err(invalid)?)
    } else {
        None
    };
    let before = a.clone();
    let previous_selected: Option<Uuid> =
        sqlx::query_scalar("SELECT selected_prompt_id FROM harnesses WHERE id=$1")
            .bind(h)
            .fetch_one(&mut *tx)
            .await
            .map_err(database)?;
    authorize_members(&mut tx, actor, h, head).await?;
    if let Some(s) = &r.changes.model_selection {
        let old = a["model_manifest"]
            .as_array()
            .ok_or_else(|| invalid("manifest"))?;
        a["model_manifest"] = snapshots(&mut tx, actor, s, old).await?;
        a["default_model_id"] = json!(s.default_model_id);
    }
    if let Some(c) = &r.changes.compute_selection {
        let previous = a["compute_manifest"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        a["compute_manifest"] = compute_snapshots(&mut tx, actor, c, &previous).await?;
        a["default_compute_id"] = json!(c.default_compute_id);
        a["contract_version"] = json!(1);
    } else if upgraded || head.is_none() {
        return Err(invalid("legacy upgrade requires compute"));
    }
    let selected_prompt = if let Some(p) = &r.changes.prompt {
        a["prompt_provenance"] = prompt(&mut tx, h, p).await?;
        a["system_prompt"] = json!(p.system_prompt);
        Some(p.library_prompt_id)
    } else {
        None
    };
    if let Some(runtime) = &r.changes.runtime {
        a["runtime_version"] = json!(runtime.runtime_version);
        a["tool_manifest"] = json!(runtime.tool_manifest);
        a["runtime_configuration"] = runtime.runtime_configuration.clone();
    }
    if r.changes.runtime.is_some()
        && a.get("compute_manifest").is_some()
        && a["runtime_configuration"]["state"] == "configured"
        && a["runtime_version"] == "interactive-v1"
    {
        let chosen = a["compute_manifest"]
            .as_array()
            .ok_or_else(|| invalid("compute"))?
            .iter()
            .find(|e| e["compute_id"] == a["default_compute_id"])
            .ok_or_else(|| invalid("compute default"))?;
        if chosen["snapshot"]["configuration"].get("image").is_some()
            && a["runtime_configuration"]["sandbox_image"]
                != chosen["snapshot"]["configuration"]["image"]
        {
            return Err(invalid("conflicting runtime image"));
        }
    }
    compute_projection(&mut a)?;
    projection(&mut a)?;
    // New revisions use the public baseline, including edits to retained local artifacts.
    a["contract_version"] = json!(1);
    let preliminary = artifacts::seal(&a).map_err(invalid)?;
    let envelope_changed = if let Some(g) = &guard {
        let previous: Option<Value> =
            sqlx::query_scalar("SELECT document FROM agent_version_specs WHERE revision_id=$1")
                .bind(head)
                .fetch_optional(&mut *tx)
                .await
                .map_err(database)?;
        previous.as_ref() != Some(&g.document)
    } else {
        false
    };
    let changed = envelope_changed
        || old.as_ref() != Some(&artifacts::configuration_digest(&preliminary).map_err(invalid)?);
    let revision = if changed {
        Uuid::new_v4()
    } else {
        head.unwrap()
    };
    let a = if changed {
        a["harness_revision_id"] = json!(revision);
        let a = artifacts::seal(&a).map_err(invalid)?;
        insert(&mut tx, h, revision, head, &a).await?;
        sqlx::query("UPDATE harnesses SET definition_revision_id=$2 WHERE id=$1")
            .bind(h)
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
        a
    } else {
        artifact(&mut tx, h, revision).await?
    };
    if let Some(prompt) = selected_prompt {
        sqlx::query("UPDATE harnesses SET selected_prompt_id=$2 WHERE id=$1")
            .bind(h)
            .bind(prompt)
            .execute(&mut *tx)
            .await
            .map_err(database)?;
    }
    if let Some(g) = &guard {
        sqlx::query("INSERT INTO agent_version_specs(agent_id,revision_id,document,implementation_kind,adapter,adapter_version,implementation_digest,created_by) VALUES($1,$2,$3,'builtin','magicmtn-interactive','1',$4,$5) ON CONFLICT DO NOTHING")
            .bind(h).bind(revision).bind(&g.document).bind(None::<String>).bind(actor.reference()).execute(&mut *tx).await.map_err(database)?;
    }
    let previous_active = active;
    let (d, active, reason) = activate_in(
        &mut tx,
        actor,
        h,
        &a,
        r.activation,
        r.expected_active_revision_id,
        d,
        active,
        &r.idempotency_key,
    )
    .await?;
    let result = response(
        h,
        revision,
        &a,
        d,
        active,
        if changed { "created" } else { "unchanged" },
        reason,
    )?;
    let mut surfaces = Vec::new();
    if changed {
        for (field, surface) in [
            ("model_manifest", "models"),
            ("compute_manifest", "compute"),
            ("default_compute_id", "compute"),
            ("default_model_id", "default"),
            ("system_prompt", "prompt"),
            ("tool_manifest", "tools"),
            ("runtime_version", "runtime"),
            ("runtime_configuration", "runtime"),
        ] {
            if before[field] != a[field] && !surfaces.contains(&surface) {
                surfaces.push(surface);
            }
        }
    }
    if selected_prompt.is_some_and(|selected| selected != previous_selected) {
        surfaces.push("prompt_selection");
    }
    if active != previous_active {
        surfaces.push("activation");
    }
    receipt(
        &mut tx,
        actor,
        "save",
        &resource,
        &r.idempotency_key,
        &hash,
        h,
        &result,
        surfaces,
    )
    .await?;
    if guard.is_some() {
        sqlx::query("UPDATE agent_drafts SET base_revision_id=$2,expected_active_revision_id=$3 WHERE agent_id=$1").bind(h).bind(revision).bind(active).execute(&mut *tx).await.map_err(database)?;
    }
    tx.commit().await.map_err(database)?;
    Ok(result)
}
pub async fn activate(
    pool: &PgPool,
    actor: Actor<'_>,
    h: Uuid,
    r: ActivationRequest,
    rollback: bool,
) -> Result<Value> {
    actor.require("release", h)?;
    key(&r.idempotency_key)?;
    let kind = if rollback { "rollback" } else { "activate" };
    let resource = h.to_string();
    let hash = contracts::digest(&json!(r)).map_err(invalid)?;
    let mut tx = begin(pool, actor, kind, &resource, &r.idempotency_key).await?;
    authorize_members(&mut tx, actor, h, Some(r.revision_id)).await?;
    if let Some(v) = replay(&mut tx, actor, kind, &resource, &r.idempotency_key, &hash).await? {
        tx.commit().await.map_err(database)?;
        return Ok(v);
    }
    let (head, d, active) = lock_head(&mut tx, h).await?;
    if head != r.expected_base_revision_id {
        return Err(conflict());
    }
    let a = artifact(&mut tx, h, r.revision_id).await?;
    if a.get("model_manifest").is_none() {
        return Err(invalid(
            "only configured revisions support manual transitions",
        ));
    }
    let (d, active, reason) = activate_in(
        &mut tx,
        actor,
        h,
        &a,
        Activation::Required,
        r.expected_active_revision_id,
        d,
        active,
        &r.idempotency_key,
    )
    .await?;
    let v = response(
        h,
        head.ok_or_else(conflict)?,
        &a,
        d,
        active,
        "unchanged",
        reason,
    )?;
    receipt(
        &mut tx,
        actor,
        kind,
        &resource,
        &r.idempotency_key,
        &hash,
        h,
        &v,
        vec!["activation"],
    )
    .await?;
    tx.commit().await.map_err(database)?;
    Ok(v)
}
pub async fn patch_metadata(
    pool: &PgPool,
    actor: Actor<'_>,
    h: Uuid,
    r: MetadataRequest,
) -> Result<Value> {
    actor.require("harness_edit", h)?;
    key(&r.idempotency_key)?;
    metadata(&r.name, r.description.as_deref())?;
    if r.expected_metadata_generation <= 0 {
        return Err(invalid("generation"));
    }
    let resource = h.to_string();
    let hash = contracts::digest(&json!(r)).map_err(invalid)?;
    let mut tx = begin(pool, actor, "metadata", &resource, &r.idempotency_key).await?;
    if let Some(v) = replay(
        &mut tx,
        actor,
        "metadata",
        &resource,
        &r.idempotency_key,
        &hash,
    )
    .await?
    {
        tx.commit().await.map_err(database)?;
        return Ok(v);
    }
    let row = sqlx::query(
        "SELECT name,description,metadata_generation FROM harnesses WHERE id=$1 FOR UPDATE",
    )
    .bind(h)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database)?
    .ok_or((StatusCode::NOT_FOUND, "Agent not found."))?;
    let generation: i64 = row.get("metadata_generation");
    if generation != r.expected_metadata_generation {
        return Err(conflict());
    }
    let changed = row.get::<String, _>("name") != r.name
        || row.get::<Option<String>, _>("description") != r.description;
    if changed {
        sqlx::query("UPDATE harnesses SET name=$2,description=$3,metadata_generation=metadata_generation+1 WHERE id=$1").bind(h).bind(&r.name).bind(&r.description).execute(&mut *tx).await.map_err(database)?;
    }
    let v = json!({"harness_id":h,"metadata_generation":generation+i64::from(changed),"outcome":if changed{"updated"}else{"unchanged"}});
    receipt(
        &mut tx,
        actor,
        "metadata",
        &resource,
        &r.idempotency_key,
        &hash,
        h,
        &v,
        if changed { vec!["metadata"] } else { vec![] },
    )
    .await?;
    tx.commit().await.map_err(database)?;
    Ok(v)
}

// Existing name-only forms adapt into the same closed configuration transactions.
pub struct CreateDefinition {
    pub name: String,
    pub workflow: String,
    pub description: String,
    pub system_prompt: String,
}
pub struct SaveDefinition {
    pub compute_selection: Option<ComputeSelection>,
    pub harness_id: Uuid,
    pub revision_id: Uuid,
    pub expected_base: Option<Uuid>,
    pub expected_active: Option<Uuid>,
    pub model_ids: Vec<Uuid>,
    pub create: Option<CreateDefinition>,
}
pub async fn save_local(pool: &PgPool, r: SaveDefinition) -> Result<Uuid> {
    let default = if let Some(base) = r.expected_base {
        let a: Value =
            sqlx::query_scalar("SELECT manifest FROM artifact_manifests WHERE revision_id=$1")
                .bind(base)
                .fetch_one(pool)
                .await
                .map_err(database)?;
        let previous: Uuid =
            serde_json::from_value(a["default_model_id"].clone()).map_err(invalid)?;
        if r.model_ids.contains(&previous) {
            previous
        } else {
            let old = a["model_manifest"]
                .as_array()
                .ok_or_else(|| invalid("manifest"))?;
            *r.model_ids
                .iter()
                .find(|id| {
                    old.iter().any(|e| {
                        e["model_id"] == id.to_string()
                            && (a["runtime_version"] == "unconfigured"
                                || e["snapshot"]["capabilities"]["tasks"]
                                    .as_array()
                                    .is_some_and(|t| t.iter().any(|t| t == "chat")))
                    })
                })
                .or_else(|| r.model_ids.first())
                .ok_or_else(|| invalid("empty set"))?
        }
    } else {
        *r.model_ids.first().ok_or_else(|| invalid("empty set"))?
    };
    let v = if let Some(c) = r.create {
        let compute = r
            .compute_selection
            .ok_or_else(|| invalid("explicit compute required"))?;
        create(
            pool,
            Actor::Local,
            CreateRequest {
                compute_ids: compute.compute_ids,
                default_compute_id: Some(compute.default_compute_id),
                name: c.name,
                description: (!c.description.is_empty()).then_some(c.description),
                workflow: c.workflow,
                model_ids: r.model_ids,
                default_model_id: default,
                system_prompt: c.system_prompt,
                library_prompt_id: None,
                runtime_version: "unconfigured".into(),
                tool_manifest: vec![],
                runtime_configuration: json!({"state":"unconfigured"}),
                activation: Activation::IfAuthorized,
                expected_active_revision_id: None,
                idempotency_key: r.revision_id.to_string(),
            },
            Some(r.harness_id),
        )
        .await?
    } else {
        save(
            pool,
            Actor::Local,
            r.harness_id,
            SaveRequest {
                expected_base_revision_id: r.expected_base,
                expected_active_revision_id: r.expected_active,
                changes: Changes {
                    compute_selection: r.compute_selection,
                    model_selection: Some(ModelSelection {
                        model_ids: r.model_ids,
                        default_model_id: default,
                        refresh_snapshots: false,
                    }),
                    ..Default::default()
                },
                activation: Activation::IfAuthorized,
                idempotency_key: r.revision_id.to_string(),
            },
        )
        .await?
    };
    serde_json::from_value(v["revision_id"].clone()).map_err(invalid)
}

#[derive(Clone)]
pub struct ConfigurationState {
    pub pool: PgPool,
    pub auth: crate::auth::AuthPolicy,
}
pub fn router<S: Clone + Send + Sync + 'static>(state: ConfigurationState) -> axum::Router<S> {
    use axum::routing::{get, patch, post};
    axum::Router::new()
        .route("/v1/agents", post(create_http))
        .route("/v1/agents/:id", patch(metadata_http))
        .route(
            "/v1/agents/:id/configuration",
            get(read_http).post(save_http),
        )
        .route("/v1/agents/:id/activate", post(activate_http))
        .route("/v1/agents/:id/rollback", post(rollback_http))
        // Legacy paths retain the same authentication and authorization.
        .route("/v1/harnesses", post(create_http))
        .route("/v1/harnesses/:id", patch(metadata_http))
        .route(
            "/v1/harnesses/:id/configuration",
            get(read_http).post(save_http),
        )
        .route("/v1/harnesses/:id/activate", post(activate_http))
        .route("/v1/harnesses/:id/rollback", post(rollback_http))
        .layer(axum::extract::DefaultBodyLimit::max(1048576))
        .with_state(state)
}
type Reply = std::result::Result<axum::Json<Value>, Failure>;
fn authenticate(s: &ConfigurationState, h: &axum::http::HeaderMap) -> Result<Principal> {
    s.auth.authenticate(h).map_err(|_| denied())
}
async fn create_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::Json(v): axum::Json<Value>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    let r = parse_create(v).map_err(invalid)?;
    create(&s.pool, Actor::Service(&p), r, None)
        .await
        .map(axum::Json)
}
async fn save_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    axum::Json(v): axum::Json<Value>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    let r = parse_save(v).map_err(invalid)?;
    save(&s.pool, Actor::Service(&p), id, r)
        .await
        .map(axum::Json)
}
async fn metadata_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    axum::Json(v): axum::Json<Value>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    patch_metadata(
        &s.pool,
        Actor::Service(&p),
        id,
        parse_metadata(v).map_err(invalid)?,
    )
    .await
    .map(axum::Json)
}
async fn activate_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    axum::Json(v): axum::Json<Value>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    activate(
        &s.pool,
        Actor::Service(&p),
        id,
        parse_activation(v).map_err(invalid)?,
        false,
    )
    .await
    .map(axum::Json)
}
async fn rollback_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    axum::Json(v): axum::Json<Value>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    activate(
        &s.pool,
        Actor::Service(&p),
        id,
        parse_activation(v).map_err(invalid)?,
        true,
    )
    .await
    .map(axum::Json)
}
pub async fn read_configuration(pool: &PgPool, actor: Actor<'_>, h: Uuid) -> Result<Value> {
    actor.require("harness_read", h)?;
    let mut tx = pool.begin().await.map_err(database)?;
    let (head, d, active) = lock_head(&mut tx, h).await?;
    authorize_members(&mut tx, actor, h, head).await?;
    let a = if let Some(head) = head {
        Some(artifact(&mut tx, h, head).await?)
    } else {
        None
    };
    let v = json!({"harness_id":h,"definition_revision_id":head,"active_revision_id":active,"deployment_id":d,"artifact":a});
    tx.commit().await.map_err(database)?;
    Ok(v)
}
async fn read_http(
    crate::workspace::State(s): crate::workspace::State<ConfigurationState>,
    h: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Reply {
    let p = authenticate(&s, &h)?;
    read_configuration(&s.pool, Actor::Service(&p), id)
        .await
        .map(axum::Json)
}

impl crate::workspace::WorkspaceState for ConfigurationState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
