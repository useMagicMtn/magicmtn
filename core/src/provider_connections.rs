//! Durable connection snapshots separate customer choices from provider software.
use crate::workspace::{self, State};
use anyhow::{ensure, Context};
use axum::{extract::Path, http::StatusCode, routing::get, Json, Router};
use magicmtn_provider_framework::host::{Installed, Pin, ProcessEnvironment, Registry};
use magicmtn_sdk::plugin::{Operation, ProviderManifest, Value as ProviderValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub connection_id: Uuid,
    pub organization_id: Uuid,
    pub generation: i64,
    pub connection_ref: String,
    pub pin: Pin,
    pub manifest: ProviderManifest,
    pub configuration: BTreeMap<String, String>,
    pub credential_mode: String,
    pub credential_ref: Option<String>,
    pub runner_id: Option<Uuid>,
}
impl Binding {
    /// Decode a connection snapshot already fetched within the caller's tenant scope.
    pub fn decode(row: &sqlx::postgres::PgRow) -> anyhow::Result<Self> {
        Ok(Self {
            connection_id: row.get("id"),
            organization_id: row.get("organization_id"),
            generation: row.get("generation"),
            connection_ref: row.get("connection_ref"),
            pin: serde_json::from_value(row.get("provider_pin"))?,
            manifest: serde_json::from_value(row.get("manifest"))?,
            configuration: serde_json::from_value(row.get("configuration"))?,
            credential_mode: row.get("credential_mode"),
            credential_ref: row.get("credential_ref"),
            runner_id: row.get("runner_id"),
        })
    }
}

#[derive(Clone)]
pub struct ConnectionState {
    pub pool: PgPool,
    pub registry: Registry,
}
impl workspace::WorkspaceState for ConnectionState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
type Reply = Result<Json<Value>, (StatusCode, String)>;
fn failure(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::debug!(reason=%e, "provider connection rejected");
    (
        StatusCode::CONFLICT,
        "provider connection unavailable or conflicting".into(),
    )
}
fn public_binding(binding: Binding) -> Value {
    let managed = binding.credential_mode == "managed";
    let mut value = json!(binding);
    if managed {
        if let Some(configuration) = value["configuration"].as_object_mut() {
            configuration.remove("MAGICMTN_E2B_TEMPLATES");
            configuration.remove("MAGICMTN_BOAT_SNAPSHOTS");
        }
        crate::redaction::ecr_references(&mut value);
    }
    value
}
fn public_profile(reference: String, mut source: ManagedCredential) -> Value {
    source.configuration.remove("MAGICMTN_E2B_TEMPLATES");
    source.configuration.remove("MAGICMTN_BOAT_SNAPSHOTS");
    let mut profile = json!({"reference":reference,"provider":source.provider,"configuration":source.configuration});
    crate::redaction::ecr_references(&mut profile);
    profile
}
pub fn router(state: ConnectionState) -> Router {
    Router::new()
        .route("/v1/provider-connections", get(list).post(create))
        .route("/v1/provider-connections/:id", axum::routing::patch(update))
        .route(
            "/v1/provider-preferences/:client",
            get(preference).put(set_preference),
        )
        .with_state(state)
}
async fn list(State(s): State<ConnectionState>) -> Reply {
    let rows = sqlx::query("SELECT * FROM provider_connections ORDER BY display_name,id")
        .fetch_all(&s.pool)
        .await
        .map_err(failure)?;
    let items = rows.iter().filter(|row| crate::public_providers::contains(&row.get::<String,_>("provider"))).map(|row| Binding::decode(row).map(|binding| json!({"binding":public_binding(binding),"display_name":row.get::<String,_>("display_name"),"status":row.get::<String,_>("status")})))
        .collect::<anyhow::Result<Vec<_>>>().map_err(failure)?;
    let managed_profiles: Vec<_> = credentials()
        .map_err(failure)?
        .into_iter()
        .filter(|(_, source)| {
            !source.provider_account.is_some()
                && crate::public_providers::contains(&source.provider)
        })
        .map(|(reference, source)| public_profile(reference, source))
        .collect();
    Ok(Json(
        json!({"connections":items,"managed_profiles":managed_profiles}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionInput {
    display_name: String,
    provider: String,
    version: String,
    sha256: Option<String>,
    #[serde(default)]
    configuration: BTreeMap<String, String>,
    credential_ref: Option<String>,
    runner_id: Option<Uuid>,
    #[serde(default = "enabled")]
    status: String,
    expected_generation: Option<i64>,
}
fn enabled() -> String {
    "enabled".into()
}

// Configuration is non-secret and never controls executable/interpreter paths.
// New providers can declare additional keys in their immutable release catalog.
pub fn configuration_keys(provider: &str) -> &'static [&'static str] {
    match provider {
        "apple-container" => &["MAGICMTN_APPLE_CONTAINER_NETWORK"],
        "firecracker" => &["MAGICMTN_FIRECRACKER_IMAGES"],
        "aws" => &[
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "MAGICMTN_AWS_BEDROCK_ROLE_ARN",
            "MAGICMTN_AWS_MICROVM_IMAGES",
            "MAGICMTN_AWS_MICROVM_EGRESS_CONNECTORS",
            "MAGICMTN_AWS_MICROVM_EXECUTION_ROLE_ARN",
        ],
        "fireworks" => &[
            "FIREWORKS_ACCOUNT_ID",
            "MAGICMTN_FIREWORKS_COLD_START_WAIT_SECONDS",
        ],
        "modal" => &["MAGICMTN_MODAL_APP", "MAGICMTN_MODAL_PREPARED_IMAGES"],
        "e2b" => &["MAGICMTN_E2B_TEMPLATES"],
        "together" => &["MAGICMTN_TOGETHER_SNAPSHOTS"],
        "daytona" => &[
            "MAGICMTN_DAYTONA_SNAPSHOTS",
            "DAYTONA_TARGET",
            "MAGICMTN_DAYTONA_USER",
        ],
        "boat" => &[
            "MAGICMTN_BOAT_SNAPSHOTS",
            "MAGICMTN_BOAT_INFERENCE_SANDBOX",
            "MAGICMTN_BOAT_INFERENCE_PORT",
        ],
        "exe" => &["MAGICMTN_EXE_POOL", "MAGICMTN_EXE_STANDALONE"],
        "openai-compatible" => &["MAGICMTN_OPENAI_BASE_URL"],
        _ => &[],
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedCredential {
    provider: String,
    secret_env: Option<String>,
    #[serde(default)]
    workload_role: bool,
    #[serde(default)]
    provider_account: Option<String>,
    #[serde(default)]
    configuration: BTreeMap<String, String>,
}
fn credentials() -> anyhow::Result<BTreeMap<String, ManagedCredential>> {
    serde_json::from_str(
        &std::env::var("MAGICMTN_MANAGED_CREDENTIALS").unwrap_or_else(|_| "{}".into()),
    )
    .context("invalid managed credential catalog")
}

/// Cloud releases opt into managed defaults explicitly. The cache avoids a
/// database write on every authenticated request; a new process reconciles
/// again, without undoing a workspace's disabled providers or connections.
#[derive(Clone)]
pub struct CloudDefaults {
    providers: Vec<(Installed, String, BTreeMap<String, String>)>,
    seen: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<Uuid>>>,
}

impl CloudDefaults {
    pub fn from_environment(registry: &Registry) -> anyhow::Result<Option<Self>> {
        let Ok(raw) = std::env::var("MAGICMTN_CLOUD_DEFAULT_PROVIDERS") else {
            return Ok(None);
        };
        ensure!(
            std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() == Ok("container")
                && registry.immutable(),
            "cloud defaults require an immutable container provider catalog"
        );
        let names: Vec<String> = serde_json::from_str(&raw)
            .context("MAGICMTN_CLOUD_DEFAULT_PROVIDERS must be a JSON array")?;
        ensure!(!names.is_empty(), "cloud default providers cannot be empty");
        let profiles = credentials()?;
        let mut unique = std::collections::HashSet::new();
        let mut providers = Vec::new();
        for name in names {
            ensure!(
                matches!(
                    name.as_str(),
                    "modal" | "e2b" | "boat" | "fireworks" | "baseten" | "aws"
                ) && unique.insert(name.clone()),
                "unsupported or duplicate cloud default provider"
            );
            let release = registry.active(&name)?;
            ensure!(
                registry.executes_on_host(&release.pin()),
                "cloud default provider requires a connected runner"
            );
            let reference = format!("managed:{name}");
            let profile = profiles
                .get(&reference)
                .context("cloud default provider needs its managed credential profile")?;
            credential_exists(&reference, &name)?;
            if !profile.workload_role && !profile.provider_account.is_some() {
                let secret_env = profile
                    .secret_env
                    .as_deref()
                    .context("managed secret missing")?;
                ensure!(
                    std::env::var(secret_env).is_ok_and(|value| !value.is_empty()),
                    "managed provider secret is unavailable"
                );
            }
            if name == "aws" {
                ensure!(
                    profile.workload_role
                        && profile.secret_env.is_none()
                        && profile.configuration.get("AWS_REGION").map(String::as_str)
                            == Some("us-east-2")
                        && profile
                            .configuration
                            .contains_key("MAGICMTN_AWS_BEDROCK_ROLE_ARN")
                        && release.manifest.capabilities.sandbox_operations.is_empty()
                        && release
                            .manifest
                            .capabilities
                            .inference_deployment_operations
                            .is_empty(),
                    "managed AWS requires a Bedrock-only release and data-plane workload role"
                );
            }
            let mut input = ConnectionInput {
                display_name: name.clone(),
                provider: name,
                version: release.package.version.clone(),
                sha256: Some(release.package.sha256.clone()),
                configuration: profile.configuration.clone(),
                credential_ref: Some(reference.clone()),
                runner_id: None,
                status: enabled(),
                expected_generation: None,
            };
            validate_input(
                &mut input,
                registry,
                false,
                profile.provider_account.is_some(),
            )?;
            let reference = match profile.provider_account.as_deref() {
                Some(account) => {
                    crate::provider_credentials::managed_reference(&reference, account)?
                }
                None => reference,
            };
            providers.push((release, reference, input.configuration));
        }
        Ok(Some(Self {
            providers,
            seen: Default::default(),
        }))
    }

    pub async fn ensure_workspace(&self, pool: &PgPool, workspace: Uuid) -> anyhow::Result<()> {
        if self.seen.lock().unwrap().contains(&workspace) {
            return Ok(());
        }
        for (release, credential_ref, configuration) in &self.providers {
            let name = &release.package.name;
            let mut tx = pool.begin().await?;
            sqlx::query("INSERT INTO organization_providers(organization_id,provider,version) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(workspace).bind(name).bind(&release.package.version)
                .execute(&mut *tx).await?;
            let status: String = sqlx::query_scalar("SELECT status FROM organization_providers WHERE organization_id=$1 AND provider=$2 FOR UPDATE")
                .bind(workspace).bind(name).fetch_one(&mut *tx).await?;
            if status != "enabled" {
                tx.commit().await?;
                continue;
            }
            let has_connection: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_connections WHERE organization_id=$1 AND provider=$2)")
                .bind(workspace).bind(name).fetch_one(&mut *tx).await?;
            if has_connection {
                tx.commit().await?;
                continue;
            }
            let id = Uuid::new_v4();
            let reference = format!("connection:cloud-{name}");
            let display_name = match name.as_str() {
                "modal" => "Modal",
                "e2b" => "E2B",
                "boat" => "Boat",
                "fireworks" => "Fireworks AI",
                "baseten" => "Baseten",
                "aws" => "AWS Bedrock",
                _ => unreachable!(),
            };
            sqlx::query("INSERT INTO provider_connections(id,organization_id,connection_ref,display_name,provider,provider_pin,manifest,configuration,credential_ref) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(id).bind(workspace).bind(&reference).bind(display_name).bind(name)
                .bind(json!(release.pin())).bind(json!(release.manifest))
                .bind(json!(configuration)).bind(credential_ref)
                .execute(&mut *tx).await?;
            if !matches!(name.as_str(), "e2b" | "boat")
                && release
                    .manifest
                    .capabilities
                    .inference_tasks
                    .iter()
                    .any(|task| task == "chat")
            {
                for mut offering in crate::library::model_offerings(name)? {
                    offering.connection_ref = Some(reference.clone());
                    sqlx::query("INSERT INTO tenant_models(id,organization_id,source,source_model_id,display_name,description,task_type,model_configuration,connection_ref,capabilities,status) VALUES(gen_random_uuid(),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING")
                        .bind(workspace).bind(&offering.source).bind(&offering.source_model_id)
                        .bind(&offering.display_name).bind(&offering.description).bind(&offering.task_type)
                        .bind(json!(offering.model_configuration)).bind(&offering.connection_ref)
                        .bind(json!(offering.capabilities)).bind(&offering.status).execute(&mut *tx).await?;
                }
            }
            let operations: Vec<_> = release
                .manifest
                .capabilities
                .sandbox_operations
                .iter()
                .filter(|operation| {
                    matches!(
                        operation,
                        magicmtn_sdk::plugin::SandboxOperation::Create
                            | magicmtn_sdk::plugin::SandboxOperation::ExecBounded
                            | magicmtn_sdk::plugin::SandboxOperation::Terminate
                    )
                })
                .collect();
            if !operations.is_empty() {
                sqlx::query("INSERT INTO tenant_compute(id,organization_id,display_name,backend,binding_ref,configuration,capabilities,status) VALUES($1,$2,$3,$4,$5,'{}',$6,'enabled') ON CONFLICT DO NOTHING")
                    .bind(id).bind(workspace).bind(display_name).bind(name)
                    .bind(format!("binding:{reference}"))
                    .bind(json!({"completeness":"partial","operations":operations}))
                    .execute(&mut *tx).await?;
            }
            if name == "modal" && !operations.is_empty() {
                sqlx::query("INSERT INTO provider_preferences(organization_id,client_kind,sandbox_connection_id) VALUES($1,'default',$2) ON CONFLICT DO NOTHING")
                    .bind(workspace).bind(id).execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }
        let mut seen = self.seen.lock().unwrap();
        if seen.len() >= 10_000 {
            seen.clear();
        }
        seen.insert(workspace);
        Ok(())
    }
}
fn credential_exists(reference: &str, provider: &str) -> anyhow::Result<()> {
    let map = credentials()?;
    let source = map
        .get(reference)
        .context("unknown managed credential reference")?;
    let valid_source = if source.provider_account.is_some() {
        !source.workload_role && source.secret_env.is_none()
    } else {
        source
            .secret_env
            .as_ref()
            .is_some_and(|value| value.starts_with("MAGICMTN_SECRET_"))
            || (source.workload_role && provider == "aws")
    };
    ensure!(
        source.provider == provider && valid_source,
        "credential provider/source mismatch"
    );
    Ok(())
}
fn validate_input(
    input: &mut ConnectionInput,
    registry: &Registry,
    allow_local_reference: bool,
    allow_prepared: bool,
) -> anyhow::Result<Installed> {
    ensure!(
        !input.display_name.trim().is_empty()
            && input.display_name.len() <= 512
            && !input.display_name.chars().any(char::is_control)
            && ["enabled", "disabled"].contains(&input.status.as_str()),
        "invalid connection"
    );
    let local_reference = input
        .credential_ref
        .as_ref()
        .is_some_and(|r| r == &format!("local:{}", input.provider));
    ensure!(
        !local_reference
            || (allow_local_reference
                && std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() != Ok("container")),
        "local credential references can only be retained by their existing local connection"
    );
    let release = match &input.sha256 {
        Some(sha256) => registry.installed_pin(&Pin {
            name: input.provider.clone(),
            version: input.version.clone(),
            sha256: sha256.clone(),
        })?,
        None => registry.installed(&input.provider, &input.version)?,
    };
    if std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() == Ok("container")
        && ["apple-container", "microsoft-mxc", "ollama"].contains(&input.provider.as_str())
    {
        ensure!(
            input.runner_id.is_some(),
            "local runtime providers require a connected runner in container mode"
        );
    }
    if input.credential_ref.is_none()
        && input.runner_id.is_none()
        && release.manifest.credential.is_none()
    {
        let profiles: Vec<_> = credentials()?
            .into_iter()
            .filter(|(_, source)| {
                source.provider == input.provider && !source.provider_account.is_some()
            })
            .map(|(reference, _)| reference)
            .collect();
        ensure!(profiles.len() <= 1, "select a managed credential profile");
        input.credential_ref = profiles.into_iter().next();
    }
    ensure!(
        input.runner_id.is_some() || registry.executes_on_host(&release.pin()),
        "this release requires a provider runner"
    );
    if release.manifest.credential.is_some() {
        ensure!(
            allow_prepared && input.runner_id.is_none() && input.credential_ref.is_some(),
            "credential-aware connections require operator provisioning"
        );
    }
    if let Some(reference) = input.credential_ref.as_ref().filter(|_| !local_reference) {
        credential_exists(reference, &input.provider)?;
        let map = credentials()?;
        let source = &map[reference];
        ensure!(
            source.provider_account.is_some() == release.manifest.credential.is_some(),
            "credential profile and provider protocol differ"
        );
        if input.provider == "openai-compatible"
            && input.configuration.contains_key("MAGICMTN_OPENAI_BASE_URL")
        {
            ensure!(
                source
                    .configuration
                    .contains_key("MAGICMTN_OPENAI_BASE_URL"),
                "managed credentials require an operator-approved endpoint"
            );
        }
        for (key, value) in &source.configuration {
            ensure!(
                input.configuration.get(key).is_none_or(|v| v == value),
                "managed connection configuration differs from credential scope"
            );
            input.configuration.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in &input.configuration {
        ensure!(
            release
                .manifest
                .credential
                .as_ref()
                .is_none_or(|spec| !spec.fields.contains(key)),
            "credential fields belong in the credential store"
        );
        ensure!(
            safe_environment_key(key)
                && ![
                    "_API_KEY",
                    "_TOKEN",
                    "_SECRET",
                    "_PASSWORD",
                    "_ACCESS_KEY_ID"
                ]
                .iter()
                .any(|suffix| key.ends_with(suffix)),
            "secret or unsafe configuration key"
        );
        ensure!(
            configuration_keys(&input.provider).contains(&key.as_str())
                || registry
                    .configuration_keys(&input.provider, &input.version)
                    .contains(key),
            "unsupported configuration key"
        );
        ensure!(
            value.len() <= 16_384 && !value.contains('\0'),
            "invalid configuration value"
        );
    }
    if let Some(reference) = input.credential_ref.as_ref().filter(|_| !local_reference) {
        credential_exists(reference, &input.provider)?;
    }
    ensure!(
        input.runner_id.is_none() || input.credential_ref.is_none(),
        "local runners supply their own credentials"
    );
    Ok(release)
}
async fn create(State(s): State<ConnectionState>, Json(mut input): Json<ConnectionInput>) -> Reply {
    crate::public_providers::require(&input.provider)?;
    let release = validate_input(&mut input, &s.registry, false, false).map_err(failure)?;
    let id = Uuid::new_v4();
    let reference = format!("connection:{id}");
    let mut tx = s.pool.begin().await.map_err(failure)?;
    sqlx::query("INSERT INTO organization_providers(organization_id,provider,version) VALUES(magicmtn_current_organization(),$1,$2) ON CONFLICT(organization_id,provider) DO UPDATE SET status='enabled'")
        .bind(&input.provider).bind(&input.version).execute(&mut *tx).await.map_err(failure)?;
    let row=sqlx::query("INSERT INTO provider_connections(id,connection_ref,display_name,provider,provider_pin,manifest,configuration,credential_ref,runner_id,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING *")
        .bind(id).bind(reference).bind(&input.display_name).bind(&input.provider).bind(json!(release.pin())).bind(json!(release.manifest))
        .bind(json!(input.configuration)).bind(&input.credential_ref).bind(input.runner_id).bind(&input.status).fetch_one(&mut *tx).await.map_err(failure)?;
    if release
        .manifest
        .capabilities
        .inference_tasks
        .iter()
        .any(|task| task == "chat")
    {
        for mut offering in crate::library::model_offerings(&input.provider).map_err(failure)? {
            offering.connection_ref = Some(format!("connection:{id}"));
            sqlx::query("INSERT INTO tenant_models(id,source,source_model_id,display_name,description,task_type,model_configuration,connection_ref,capabilities,status) VALUES(gen_random_uuid(),$1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(&offering.source).bind(&offering.source_model_id).bind(&offering.display_name).bind(&offering.description).bind(&offering.task_type)
            .bind(json!(offering.model_configuration)).bind(&offering.connection_ref).bind(json!(offering.capabilities))
            .bind(if input.status == "enabled" { &offering.status } else { &input.status }).execute(&mut *tx).await.map_err(failure)?;
        }
    }
    let operations: Vec<_> = release
        .manifest
        .capabilities
        .sandbox_operations
        .iter()
        .filter(|op| {
            matches!(
                op,
                magicmtn_sdk::plugin::SandboxOperation::Create
                    | magicmtn_sdk::plugin::SandboxOperation::ExecBounded
                    | magicmtn_sdk::plugin::SandboxOperation::Terminate
            )
        })
        .collect();
    if !operations.is_empty() {
        sqlx::query("INSERT INTO tenant_compute(id,display_name,backend,binding_ref,configuration,capabilities,status) VALUES($1,$2,$3,$4,'{}',$5,$6)")
            .bind(id).bind(&input.display_name).bind(&input.provider).bind(format!("binding:connection:{id}"))
            .bind(json!({"completeness":"partial","operations":operations})).bind(&input.status).execute(&mut *tx).await.map_err(failure)?;
    }
    if input.provider == "apple-container" && input.status == "enabled" {
        sqlx::query("INSERT INTO provider_preferences(organization_id,client_kind,sandbox_connection_id) VALUES($1,'desktop',$2) ON CONFLICT DO NOTHING")
            .bind(row.get::<Uuid,_>("organization_id")).bind(id).execute(&mut *tx).await.map_err(failure)?;
    }
    tx.commit().await.map_err(failure)?;
    Ok(Json(public_binding(
        Binding::decode(&row).map_err(failure)?,
    )))
}
async fn update(
    State(s): State<ConnectionState>,
    Path(id): Path<Uuid>,
    Json(mut input): Json<ConnectionInput>,
) -> Reply {
    crate::public_providers::require(&input.provider)?;
    let retain_local:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_connections WHERE id=$1 AND credential_ref=$2 AND provider=$3)")
        .bind(id).bind(&input.credential_ref).bind(&input.provider).fetch_one(&s.pool).await.map_err(failure)?;
    let release = validate_input(&mut input, &s.registry, retain_local, false).map_err(failure)?;
    let row=sqlx::query("UPDATE provider_connections SET display_name=$2,provider_pin=$3,manifest=$4,configuration=$5,credential_ref=$6,runner_id=$7,status=$8,generation=generation+1,updated_at=now() WHERE id=$1 AND provider=$9 AND generation=$10 RETURNING *")
        .bind(id).bind(&input.display_name).bind(json!(release.pin())).bind(json!(release.manifest)).bind(json!(input.configuration))
        .bind(&input.credential_ref).bind(input.runner_id).bind(&input.status).bind(&input.provider).bind(input.expected_generation)
        .fetch_optional(&s.pool).await.map_err(failure)?.ok_or_else(||failure("generation conflict"))?;
    Ok(Json(public_binding(
        Binding::decode(&row).map_err(failure)?,
    )))
}
async fn preference(State(s): State<ConnectionState>, Path(client): Path<String>) -> Reply {
    let id: Option<Uuid> = sqlx::query_scalar(
        "SELECT sandbox_connection_id FROM provider_preferences WHERE client_kind=$1",
    )
    .bind(client)
    .fetch_optional(&s.pool)
    .await
    .map_err(failure)?;
    Ok(Json(json!({"sandbox_connection_id":id})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    sandbox_connection_id: Uuid,
}
async fn set_preference(
    State(s): State<ConnectionState>,
    Path(client): Path<String>,
    Json(body): Json<Preference>,
) -> Reply {
    let b = by_id(&s.pool, body.sandbox_connection_id)
        .await
        .map_err(failure)?;
    crate::public_providers::require(&b.pin.name)?;
    ensure_sandbox(&b).map_err(failure)?;
    sqlx::query("INSERT INTO provider_preferences(organization_id,client_kind,sandbox_connection_id) VALUES($1,$2,$3) ON CONFLICT(organization_id,client_kind) DO UPDATE SET sandbox_connection_id=EXCLUDED.sandbox_connection_id")
        .bind(b.organization_id).bind(client).bind(b.connection_id).execute(&s.pool).await.map_err(failure)?;
    Ok(Json(json!({"sandbox_connection_id":b.connection_id})))
}
pub fn ensure_sandbox(binding: &Binding) -> anyhow::Result<()> {
    use magicmtn_sdk::plugin::SandboxOperation::*;
    ensure!(
        [Create, ExecBounded, Terminate].iter().all(|op| binding
            .manifest
            .capabilities
            .sandbox_operations
            .contains(op)),
        "connection cannot execute sandbox sessions"
    );
    Ok(())
}
pub async fn by_id(pool: &PgPool, id: Uuid) -> anyhow::Result<Binding> {
    let row = sqlx::query("SELECT * FROM provider_connections WHERE id=$1 AND status='enabled'")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Binding::decode(&row)
}
pub async fn by_ref(
    pool: &PgPool,
    org: Uuid,
    reference: &str,
    provider: &str,
) -> anyhow::Result<Binding> {
    let row=sqlx::query("SELECT * FROM provider_connections WHERE organization_id=$1 AND connection_ref=$2 AND provider=$3 AND status='enabled'")
        .bind(org).bind(reference.strip_prefix("binding:connection:").map(|v|format!("connection:{v}")).unwrap_or_else(||reference.to_owned())).bind(provider).fetch_one(pool).await?;
    Binding::decode(&row)
}
pub async fn by_ref_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
    reference: &str,
    provider: &str,
) -> anyhow::Result<Binding> {
    let reference = reference
        .strip_prefix("binding:connection:")
        .map(|v| format!("connection:{v}"))
        .unwrap_or_else(|| reference.to_owned());
    let row=sqlx::query("SELECT * FROM provider_connections WHERE organization_id=$1 AND connection_ref=$2 AND provider=$3 AND status='enabled'")
        .bind(org).bind(reference).bind(provider).fetch_one(&mut **tx).await?;
    Binding::decode(&row)
}
pub async fn default_sandbox(pool: &PgPool, client: &str) -> anyhow::Result<Option<Binding>> {
    let id:Option<Uuid>=sqlx::query_scalar("SELECT p.sandbox_connection_id FROM provider_preferences p JOIN provider_connections c ON c.id=p.sandbox_connection_id WHERE p.client_kind IN ($1,'default') AND c.status='enabled' ORDER BY (p.client_kind=$1) DESC LIMIT 1")
        .bind(client).fetch_optional(pool).await?;
    match id {
        Some(id) => Ok(Some(by_id(pool, id).await?)),
        None => Ok(None),
    }
}

/// Restore old installation references into durable connection rows once. Cloud
/// mode never invents global connections or imports the server's whole environment.
pub async fn bootstrap_local(pool: &PgPool, registry: &Registry) -> anyhow::Result<()> {
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM platform_workspace")
        .fetch_one(pool)
        .await?;
    bootstrap_organization_inner(pool, registry, org, true).await
}
pub async fn bootstrap_organization(
    pool: &PgPool,
    registry: &Registry,
    org: Uuid,
) -> anyhow::Result<()> {
    bootstrap_organization_inner(pool, registry, org, false).await
}
async fn bootstrap_organization_inner(
    pool: &PgPool,
    registry: &Registry,
    org: Uuid,
    installed: bool,
) -> anyhow::Result<()> {
    let registrations: Vec<(String, String)> = sqlx::query_as("SELECT provider,version FROM organization_providers WHERE organization_id=$1 AND status='enabled'").bind(org).fetch_all(pool).await?;
    let releases = if installed || registrations.is_empty() {
        registry
            .list()?
            .into_iter()
            .filter(|r| r.active)
            .collect::<Vec<_>>()
    } else {
        registrations
            .into_iter()
            .map(|(name, version)| {
                if version.is_empty() {
                    registry.active(&name)
                } else {
                    registry.installed(&name, &version)
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    for release in releases.into_iter().filter(|r| {
        r.package.os == std::env::consts::OS && r.package.arch == std::env::consts::ARCH
    }) {
        if release.manifest.credential.is_some() {
            continue;
        }
        let name = &release.package.name;
        sqlx::query("INSERT INTO organization_providers(organization_id,provider,version) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(org).bind(name).bind(&release.package.version).execute(pool).await?;
        let mut refs:Vec<String>=sqlx::query_scalar("SELECT binding_ref FROM tenant_compute WHERE organization_id=$1 AND backend=$2 UNION SELECT connection_ref FROM tenant_models WHERE organization_id=$1 AND model_configuration->>'provider'=$2 AND connection_ref IS NOT NULL")
            .bind(org).bind(name).fetch_all(pool).await?;
        refs.push(format!("binding:provider:{name}"));
        refs.push(format!("connection:self-{name}"));
        for reference in refs {
            let id = Uuid::new_v4();
            let configuration = local_configuration(name);
            sqlx::query("INSERT INTO provider_connections(id,organization_id,connection_ref,display_name,provider,provider_pin,manifest,configuration,credential_ref) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(organization_id,connection_ref) DO NOTHING")
                .bind(id).bind(org).bind(reference).bind(name).bind(name).bind(json!(release.pin())).bind(json!(release.manifest)).bind(json!(configuration))
                .bind(format!("local:{name}")).execute(pool).await?;
        }
    }
    sqlx::query("INSERT INTO provider_preferences(organization_id,client_kind,sandbox_connection_id) SELECT $1,'desktop',id FROM provider_connections WHERE organization_id=$1 AND provider='apple-container' ORDER BY created_at,id LIMIT 1 ON CONFLICT DO NOTHING")
        .bind(org).execute(pool).await?;
    // Ended sessions retain historical pins and need no runnable connection.
    // Their provider bytes may no longer be installed on this host.
    let sessions=sqlx::query("SELECT s.id,s.provider_bindings,s.compute_snapshot,s.model_snapshot,a.manifest FROM chat_sessions s JOIN artifact_manifests a ON a.revision_id=s.revision_id WHERE s.organization_id=$1 AND s.state<>'ended' AND s.provider_bindings IS NOT NULL AND NOT (s.provider_bindings ? 'sandbox_connection')")
        .bind(org).fetch_all(pool).await?;
    for session in sessions {
        let mut pins: Value = session.get("provider_bindings");
        let compute: Option<Value> = session.get("compute_snapshot");
        let model: Option<Value> = session.get("model_snapshot");
        let artifact: Value = session.get("manifest");
        let inferred = model
            .as_ref()
            .and_then(|m| m["connection_ref"].as_str())
            .or_else(|| {
                artifact["model_manifest"]
                    .as_array()
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|e| e["model_id"] == artifact["default_model_id"])
                    })
                    .and_then(|e| e["snapshot"]["connection_ref"].as_str())
            });
        let reference = compute
            .as_ref()
            .and_then(|c| c["binding_ref"].as_str())
            .unwrap_or("binding:provider:apple-container");
        let Some(inferred) = inferred else {
            continue;
        };
        let mut bindings = Vec::new();
        for (slot, reference) in [("inference", inferred), ("sandbox", reference)] {
            let pin: Pin = serde_json::from_value(pins[slot].clone())?;
            let mut binding = by_ref(pool, org, reference, &pin.name).await?;
            let release = registry.installed_pin(&pin)?;
            ensure!(release.pin() == pin, "retained provider digest differs");
            binding.pin = pin;
            binding.manifest = release.manifest;
            bindings.push((format!("{slot}_connection"), json!(binding)));
        }
        for (key, binding) in bindings {
            pins[&key] = binding;
        }
        sqlx::query("UPDATE chat_sessions SET provider_bindings=$2 WHERE id=$1")
            .bind(session.get::<Uuid, _>("id"))
            .bind(pins)
            .execute(pool)
            .await?;
    }
    sqlx::query("UPDATE sandboxes b SET metadata=COALESCE(b.metadata,'{}')||jsonb_build_object('_magicmtn_connection',s.provider_bindings->'sandbox_connection') FROM chat_sessions s WHERE s.sandbox_id=b.id AND b.organization_id=$1 AND s.provider_bindings ? 'sandbox_connection' AND NOT (COALESCE(b.metadata,'{}') ? '_magicmtn_connection')")
        .bind(org).execute(pool).await?;
    Ok(())
}

/// Refuse a deployment that omitted a release needed by existing durable state.
pub async fn validate_catalog_references(pool: &PgPool, registry: &Registry) -> anyhow::Result<()> {
    let pins:Vec<Value>=sqlx::query_scalar("SELECT provider_pin FROM provider_connections WHERE status='enabled' UNION SELECT provider_bindings->'sandbox' FROM chat_sessions WHERE provider_bindings IS NOT NULL AND state<>'ended' UNION SELECT provider_bindings->'inference' FROM chat_sessions WHERE provider_bindings IS NOT NULL AND state<>'ended' UNION SELECT metadata->'_magicmtn_provider' FROM sandboxes WHERE status IS DISTINCT FROM 'ended' AND metadata ? '_magicmtn_provider' UNION SELECT binding->'pin' FROM inference_resources WHERE state<>'deleted' UNION SELECT binding->'pin' FROM provider_resource_receipts WHERE state IN ('dispatched','uncertain')")
        .fetch_all(pool).await?;
    for pin in pins {
        if pin.is_null() {
            continue;
        }
        let pin: Pin = serde_json::from_value(pin)?;
        registry.installed_pin(&pin).with_context(|| {
            format!(
                "deployment omitted required provider {} {}",
                pin.name, pin.version
            )
        })?;
    }
    Ok(())
}

// Prepared images are an operator-maintained catalog. Allow additive entries
// without changing any image identity already pinned in a durable binding.
fn apply_managed_configuration(
    provider: &str,
    values: &mut BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for (key, value) in current {
        if values.get(key) == Some(value) {
            continue;
        }
        let compatible = if matches!(
            (provider, key.as_str()),
            ("modal", "MAGICMTN_MODAL_PREPARED_IMAGES") | ("e2b", "MAGICMTN_E2B_TEMPLATES")
        ) {
            let saved: Option<BTreeMap<String, String>> = values
                .get(key)
                .and_then(|value| serde_json::from_str(value).ok());
            let updated: Option<BTreeMap<String, String>> = serde_json::from_str(value).ok();
            matches!((saved, updated), (Some(saved), Some(updated))
                if saved.iter().all(|(image, id)| updated.get(image) == Some(id)))
        } else {
            false
        };
        ensure!(
            compatible,
            "managed credential scope changed; retain the original profile for existing bindings"
        );
        values.insert(key.clone(), value.clone());
    }
    Ok(())
}

pub fn environment(binding: &Binding) -> anyhow::Result<ProcessEnvironment> {
    ensure!(
        binding.credential_mode == "managed",
        "BYOK is not enabled yet"
    );
    let mut values = binding.configuration.clone();
    if binding.pin.name == "aws" {
        if let Some(region) = values.get("AWS_REGION").cloned() {
            values.entry("AWS_DEFAULT_REGION".into()).or_insert(region);
        }
    }
    if let Some(reference) = &binding.credential_ref {
        if reference.starts_with("local:") {
            ensure!(
                std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() != Ok("container"),
                "local credentials forbidden in cloud mode"
            );
            for key in legacy_credential_keys(&binding.pin.name) {
                if let Ok(value) = std::env::var(key) {
                    values.insert(key.to_string(), value);
                }
            }
            // Local runtime state is on the runner, never on a managed backend.
            for key in ["HOME", "MAGICMTN_MXC_STATE_DIR"] {
                if let Ok(value) = std::env::var(key) {
                    values.insert(key.into(), value);
                }
            }
            for key in [
                "MAGICMTN_AWS_PYTHON",
                "MAGICMTN_MODAL_PYTHON",
                "MAGICMTN_TOGETHER_PYTHON",
                "PYTHONPATH",
            ] {
                if let Ok(value) = std::env::var(key) {
                    values.insert(key.into(), value);
                }
            }
        } else {
            credential_exists(reference, &binding.pin.name)?;
            let map = credentials()?;
            let source = &map[reference];
            apply_managed_configuration(&binding.pin.name, &mut values, &source.configuration)?;
            if source.workload_role {
                ensure!(
                    binding.pin.name == "aws",
                    "workload role requires AWS provider"
                );
                for key in [
                    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
                    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
                ] {
                    if let Ok(value) = std::env::var(key) {
                        values.insert(key.into(), value);
                    }
                }
            }
            if let Some(secret_env) = &source.secret_env {
                let raw = std::env::var(secret_env)
                    .map_err(|_| anyhow::anyhow!("managed secret not supplied"))?;
                let bundle: BTreeMap<String, String> = serde_json::from_str(&raw)
                    .map_err(|_| anyhow::anyhow!("invalid managed secret bundle"))?;
                for (key, value) in bundle {
                    ensure!(
                        safe_environment_key(&key) && !values.contains_key(&key),
                        "unsafe or conflicting secret environment key"
                    );
                    values.insert(key, value);
                }
            }
        }
    }
    if std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() != Ok("container")
        && ["apple-container", "microsoft-mxc"].contains(&binding.pin.name.as_str())
    {
        // Apple Silicon Homebrew installs the container CLI outside the
        // framework's system-only PATH. Use fixed dependency locations rather
        // than forwarding the server's arbitrary shell PATH.
        if binding.pin.name == "apple-container" {
            values.insert(
                "PATH".into(),
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
            );
        }
        for key in ["HOME", "MAGICMTN_MXC_STATE_DIR"] {
            if let Ok(value) = std::env::var(key) {
                values.insert(key.into(), value);
            }
        }
    }
    Ok(ProcessEnvironment { values })
}
fn legacy_credential_keys(provider: &str) -> &'static [&'static str] {
    match provider {
        "aws" => &[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_PROFILE",
        ],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "baseten" => &["BASETEN_API_KEY"],
        "modal" => &["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"],
        "e2b" => &["E2B_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "daytona" => &["DAYTONA_API_KEY"],
        "boat" => &["BOAT_API_KEY"],
        "exe" => &["MAGICMTN_EXE_API_TOKEN"],
        "openai-compatible" => &["MAGICMTN_OPENAI_API_KEY", "OPENAI_API_KEY"],
        _ => &[],
    }
}
fn safe_environment_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && ![
            "PATH",
            "HOME",
            "TMPDIR",
            "PYTHONPATH",
            "PYTHONHOME",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
        ]
        .contains(&key)
        && !key.ends_with("_PYTHON")
}

/// The snapshot owns resource identity/configuration; current status can revoke
/// invocation, while cleanup may still use a disabled connection's old binding.
pub async fn dispatch(
    pool: &PgPool,
    registry: &Registry,
    binding: &Binding,
    scope: &str,
    operation: Operation,
    timeout: Duration,
    cleanup: bool,
) -> anyhow::Result<ProviderValue> {
    dispatch_store(
        &pool.clone().into(),
        registry,
        binding,
        scope,
        operation,
        timeout,
        cleanup,
    )
    .await
}

#[derive(Debug, thiserror::Error)]
#[error("provider credential unavailable")]
pub(crate) struct CredentialUnavailable;

pub(crate) fn admit_credential(
    registry: &Registry,
    binding: &Binding,
) -> anyhow::Result<Option<magicmtn_provider_framework::host::CredentialAdmission>> {
    if binding.manifest.credential.is_none() {
        return Ok(None);
    }
    let key = crate::provider_credentials::reference(binding)?;
    registry
        .admit_credential(&key, &binding.pin)
        .map(Some)
        .map_err(|_| CredentialUnavailable.into())
}

pub async fn dispatch_store(
    store: &crate::sandbox_storage::Store,
    registry: &Registry,
    binding: &Binding,
    scope: &str,
    operation: Operation,
    timeout: Duration,
    cleanup: bool,
) -> anyhow::Result<ProviderValue> {
    dispatch_prepared(
        store, registry, binding, scope, operation, timeout, cleanup, None,
    )
    .await
}

pub(crate) async fn dispatch_prepared(
    store: &crate::sandbox_storage::Store,
    registry: &Registry,
    binding: &Binding,
    scope: &str,
    operation: Operation,
    timeout: Duration,
    cleanup: bool,
    prepared: Option<magicmtn_provider_framework::host::CredentialAdmission>,
) -> anyhow::Result<ProviderValue> {
    store.validate_binding(binding, cleanup).await?;
    ensure!(
        binding.manifest.capabilities.supports(&operation),
        "unsupported provider operation"
    );
    if binding.credential_mode == "managed" {
        let model = match &operation {
            Operation::Infer { request } => Some(request.model.model.as_str()),
            Operation::InferenceDeploymentCreate { spec } => Some(spec.model.as_str()),
            _ => None,
        };
        if let (Some(model), crate::sandbox_storage::Store::Postgres(pool)) = (model, store) {
            // Only the trusted service can perform this cross-organization check.
            // A transaction-local override is rolled back before normal dispatch.
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT set_config('magicmtn.worker','on',true)")
                .execute(&mut *tx)
                .await?;
            let private:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inference_resources WHERE organization_id<>$1 AND binding#>>'{pin,name}'=$2 AND snapshot->>'inference_model'=$3 AND state<>'deleted')")
                .bind(binding.organization_id).bind(&binding.pin.name).bind(model).fetch_one(&mut *tx).await?;
            tx.rollback().await?;
            ensure!(
                !private,
                "managed serving model belongs to another organization"
            );
        }
    }
    let admitted = match prepared {
        Some(admission) => Some(admission),
        None => admit_credential(registry, binding)?,
    };
    let prepared = if let Some(admission) = admitted {
        let client_scope = format!("{scope}:{}:{}", binding.connection_id, binding.generation);
        Some(
            registry
                .prepared_client(admission, &client_scope, &binding.configuration)
                .await
                .map_err(|_| CredentialUnavailable)?,
        )
    } else {
        None
    };
    let kind = match &operation {
        Operation::SandboxCreate { .. } => Some("sandbox"),
        Operation::InferenceDeploymentCreate { .. } => Some("inference"),
        _ => None,
    };
    if let Some(kind) = kind {
        if !store.reserve_receipt(binding, scope, kind).await? {
            return serde_json::from_value(store.receipt(binding, scope, kind).await?)
                .map_err(Into::into);
        }
    }
    let result = match prepared {
        Some(client) => client.call(operation, timeout).await,
        None => dispatch_once(store, registry, binding, scope, operation, timeout).await,
    };
    if let Some(kind) = kind {
        store
            .finish_receipt(binding, scope, kind, result.as_ref().ok().map(|v| json!(v)))
            .await?;
    }
    result
}

async fn dispatch_once(
    store: &crate::sandbox_storage::Store,
    registry: &Registry,
    binding: &Binding,
    scope: &str,
    operation: Operation,
    timeout: Duration,
) -> anyhow::Result<ProviderValue> {
    if binding.runner_id.is_some() {
        match store {
            crate::sandbox_storage::Store::Postgres(pool) => {
                return crate::provider_runners::dispatch(pool, binding, scope, operation, timeout)
                    .await
            }
            #[cfg(feature = "sqlite")]
            _ => anyhow::bail!("local connections cannot use remote runners"),
        }
    }
    let environment = environment(binding)?;
    // Scope includes immutable connection generation to avoid cross-account clients.
    let scope = format!("{scope}:{}:{}", binding.connection_id, binding.generation);
    registry
        .call_configured(&binding.pin, &scope, operation, timeout, Some(&environment))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    #[test]
    fn e2b_template_additions_preserve_pinned_images() {
        let key = "MAGICMTN_E2B_TEMPLATES".to_owned();
        let saved = BTreeMap::from([(
            key.clone(),
            json!({"image@sha256:old":"template-old"}).to_string(),
        )]);
        let current = BTreeMap::from([(
            key.clone(),
            json!({"image@sha256:old":"template-old","image@sha256:new":"template-new"})
                .to_string(),
        )]);
        let mut effective = saved.clone();
        apply_managed_configuration("e2b", &mut effective, &current).unwrap();
        assert_eq!(effective, current);
        assert_eq!(
            saved[&key],
            json!({"image@sha256:old":"template-old"}).to_string()
        );
        for incompatible in [
            json!({"image@sha256:old":"replacement"}),
            json!({"image@sha256:new":"template-new"}),
        ] {
            assert!(apply_managed_configuration(
                "e2b",
                &mut saved.clone(),
                &BTreeMap::from([(key.clone(), incompatible.to_string())])
            )
            .is_err());
        }
        assert!(apply_managed_configuration("modal", &mut saved.clone(), &current).is_err());
    }

    #[test]
    fn public_binding_omits_managed_e2b_template_map() {
        let binding = Binding {
            connection_id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            generation: 1,
            connection_ref: "connection:test".into(),
            pin: Pin {
                name: "e2b".into(),
                version: "0.2.0".into(),
                sha256: "abc".into(),
            },
            manifest: ProviderManifest {
                credential: None,
                manifest_version: 1,
                provider_version: "0.2.0".into(),
                capabilities: magicmtn_sdk::plugin::Capabilities::sandbox(vec![
                    magicmtn_sdk::plugin::SandboxOperation::Create,
                    magicmtn_sdk::plugin::SandboxOperation::ExecBounded,
                    magicmtn_sdk::plugin::SandboxOperation::Terminate,
                ]),
            },
            configuration: BTreeMap::from([(
                "MAGICMTN_E2B_TEMPLATES".into(),
                "{\"123456789012.dkr.ecr.us-east-2.amazonaws.com/app@sha256:abc\":\"template-id\"}"
                    .into(),
            )]),
            credential_mode: "managed".into(),
            credential_ref: Some("managed:e2b".into()),
            runner_id: None,
        };
        let public = public_binding(binding.clone());
        assert_eq!(public["pin"]["name"], "e2b");
        assert!(public["configuration"]
            .get("MAGICMTN_E2B_TEMPLATES")
            .is_none());
        assert!(!public.to_string().contains("dkr.ecr"));
        assert!(binding.configuration.contains_key("MAGICMTN_E2B_TEMPLATES"));
        let mut custom = binding;
        custom.credential_mode = "byok".into();
        assert!(public_binding(custom)["configuration"]
            .get("MAGICMTN_E2B_TEMPLATES")
            .is_some());
    }

    #[test]
    fn public_managed_profile_omits_template_map() {
        let profile = public_profile("managed:e2b".into(), ManagedCredential {
            provider: "e2b".into(),
            secret_env: Some("MAGICMTN_SECRET_E2B".into()),
            workload_role: false,
            provider_account: None,
            configuration: BTreeMap::from([("MAGICMTN_E2B_TEMPLATES".into(),
                "{\"123456789012.dkr.ecr.us-east-2.amazonaws.com/app@sha256:abc\":\"template-id\"}".into())]),
        });
        assert_eq!(profile["reference"], "managed:e2b");
        assert!(profile["configuration"]
            .get("MAGICMTN_E2B_TEMPLATES")
            .is_none());
        assert!(!profile.to_string().contains("dkr.ecr"));
    }

    #[test]
    fn public_boat_profile_omits_snapshot_mapping() {
        let profile = public_profile(
            "managed:boat".into(),
            ManagedCredential {
                provider: "boat".into(),
                secret_env: Some("MAGICMTN_SECRET_BOAT".into()),
                workload_role: false,
                provider_account: None,
                configuration: BTreeMap::from([(
                    "MAGICMTN_BOAT_SNAPSHOTS".into(),
                    "{\"private-image@sha256:abc\":\"private-snapshot\"}".into(),
                )]),
            },
        );
        assert_eq!(profile["reference"], "managed:boat");
        assert!(profile["configuration"]
            .get("MAGICMTN_BOAT_SNAPSHOTS")
            .is_none());
        assert!(!profile.to_string().contains("private-snapshot"));
    }

    #[test]
    fn modal_catalog_can_grow_without_retargeting_saved_images() {
        let key = "MAGICMTN_MODAL_PREPARED_IMAGES".to_string();
        let saved = BTreeMap::from([
            (key.clone(), json!({"old@sha256:aaa":"im-old"}).to_string()),
            ("MAGICMTN_MODAL_APP".into(), "original-app".into()),
        ]);
        let mut current = saved.clone();
        current.insert(
            key.clone(),
            json!({"old@sha256:aaa":"im-old", "new@sha256:bbb":"im-new"}).to_string(),
        );
        let mut effective = saved.clone();
        apply_managed_configuration("modal", &mut effective, &current).unwrap();
        assert_eq!(effective, current);
        assert!(apply_managed_configuration("other", &mut saved.clone(), &current).is_err());
        for replacement in [
            json!({"old@sha256:aaa":"im-replaced"}).to_string(),
            json!({"new@sha256:bbb":"im-new"}).to_string(),
            "invalid".to_string(),
        ] {
            current.insert(key.clone(), replacement);
            assert!(apply_managed_configuration("modal", &mut saved.clone(), &current).is_err());
        }
        let mut changed_app = saved.clone();
        changed_app.insert("MAGICMTN_MODAL_APP".into(), "other-app".into());
        assert!(apply_managed_configuration("modal", &mut saved.clone(), &changed_app).is_err());
        let mut no_catalog = saved.clone();
        no_catalog.remove(&key);
        assert!(apply_managed_configuration("modal", &mut no_catalog, &saved).is_err());
    }

    #[test]
    fn managed_profile_defaults_are_persistable_and_cannot_be_redirected() {
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("MAGICMTN_MANAGED_CREDENTIALS", value),
                    None => std::env::remove_var("MAGICMTN_MANAGED_CREDENTIALS"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("MAGICMTN_MANAGED_CREDENTIALS"));
        std::env::set_var("MAGICMTN_MANAGED_CREDENTIALS",json!({"managed:fireworks":{"provider":"fireworks","secret_env":"MAGICMTN_SECRET_TEST","configuration":{"FIREWORKS_ACCOUNT_ID":"managed-account"}}}).to_string());
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("provider");
        std::fs::write(&executable, b"fixture").unwrap();
        let release = magicmtn_provider_framework::host::Release {
            installed: Installed {
                package: magicmtn_provider_framework::host::Package {
                    name: "fireworks".into(),
                    version: "0.1.0".into(),
                    protocol_version: 1,
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                    sha256: hex::encode(sha2::Sha256::digest(b"fixture")),
                },
                manifest: ProviderManifest {
                    credential: None,
                    manifest_version: 1,
                    provider_version: "0.1.0".into(),
                    capabilities: magicmtn_sdk::plugin::Capabilities::inference(
                        vec!["chat".into()],
                    ),
                },
                active: true,
            },
            executable: Some(executable),
            configuration_keys: vec![],
            runtime_environment: BTreeMap::new(),
        };
        let registry = Registry::from_releases(vec![release]).unwrap();
        let mut input = ConnectionInput {
            display_name: "Fireworks".into(),
            provider: "fireworks".into(),
            version: "0.1.0".into(),
            sha256: None,
            configuration: BTreeMap::new(),
            credential_ref: None,
            runner_id: None,
            status: "enabled".into(),
            expected_generation: None,
        };
        validate_input(&mut input, &registry, false, false).unwrap();
        assert_eq!(input.credential_ref.as_deref(), Some("managed:fireworks"));
        assert_eq!(
            input.configuration["FIREWORKS_ACCOUNT_ID"],
            "managed-account"
        );
        input
            .configuration
            .insert("FIREWORKS_ACCOUNT_ID".into(), "another-account".into());
        assert!(validate_input(&mut input, &registry, false, false).is_err());
    }
    #[test]
    fn configuration_cannot_change_programs_or_credentials() {
        for key in ["PATH", "PYTHONPATH", "LD_PRELOAD", "MAGICMTN_AWS_PYTHON"] {
            assert!(!safe_environment_key(key));
        }
        assert!(configuration_keys("modal").contains(&"MAGICMTN_MODAL_PREPARED_IMAGES"));
        assert!(configuration_keys("e2b").contains(&"MAGICMTN_E2B_TEMPLATES"));
        assert_eq!(
            configuration_keys("together"),
            &["MAGICMTN_TOGETHER_SNAPSHOTS"]
        );
        assert_eq!(legacy_credential_keys("together"), &["TOGETHER_API_KEY"]);
        assert!(configuration_keys("daytona").contains(&"MAGICMTN_DAYTONA_SNAPSHOTS"));
        assert_eq!(legacy_credential_keys("daytona"), &["DAYTONA_API_KEY"]);
        for key in ["DAYTONA_API_KEY", "DAYTONA_API_URL"] {
            assert!(!configuration_keys("daytona").contains(&key));
        }
        assert!(safe_environment_key("MAGICMTN_MODAL_PREPARED_IMAGES"));
        assert!(safe_environment_key("FIREWORKS_API_KEY"));
        assert!(!configuration_keys("fireworks").contains(&"FIREWORKS_API_KEY"));
    }
}

/// Freeze non-secret local provider configuration with the connection snapshot.
pub fn local_configuration(provider: &str) -> BTreeMap<String, String> {
    configuration_keys(provider)
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|v| (key.to_string(), v)))
        .collect()
}
