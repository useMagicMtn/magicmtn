//! Managed local sessions. Serving bindings and sandbox identities are frozen
//! independently; the relay never gives a guest the installation credential.
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use magicmtn_engine::{
    provider_connections::{dispatch_store, Binding},
    sandbox_storage::{sqlite::Database, Store},
    sandboxes::{self, Create, LocalPolicy, ServiceState},
};
use magicmtn_provider_framework::host::Registry;
use magicmtn_sdk::{
    inference::ModelConfiguration,
    plugin::{InferenceDeploymentOperation, Operation, SandboxOperation, Value as ProviderValue},
    InferenceDeployment, InferenceDeploymentSpec, InferenceDeploymentState, InferenceRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use uuid::Uuid;

const RUNTIME: &str = include_str!("../runtime/pi.py");
const SCRIPT: &str = "/computer/.magicmtn-local-pi.py";
const LIMIT: usize = 16 * 1024 * 1024;

// Never log error text here: provider errors can contain credentials or prompts.
fn error_category(error: &anyhow::Error, fallback: &'static str) -> &'static str {
    if error.downcast_ref::<sqlx::Error>().is_some() {
        "database"
    } else if error.downcast_ref::<serde_json::Error>().is_some()
        || error.downcast_ref::<base64::DecodeError>().is_some()
    {
        "protocol"
    } else {
        fallback
    }
}

struct Retry {
    interval: Duration,
    failures: u32,
}

impl Retry {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            failures: 0,
        }
    }

    fn delay(
        &mut self,
        result: Result<()>,
        session_id: Option<Uuid>,
        operation: &'static str,
    ) -> Duration {
        match result {
            Ok(()) => {
                if self.failures > 0 {
                    tracing::info!(
                        ?session_id,
                        operation,
                        failures = self.failures,
                        "Local Pi worker recovered"
                    );
                }
                self.failures = 0;
                self.interval
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                let delay =
                    (self.interval * (1_u32 << self.failures.min(8))).min(Duration::from_secs(30));
                tracing::warn!(
                    ?session_id,
                    operation,
                    category = error_category(&error, operation),
                    failures = self.failures,
                    retry_ms = delay.as_millis() as u64,
                    "Local Pi worker failed; retrying"
                );
                delay
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Service {
    db: Database,
    registry: Registry,
    ttl: u32,
    selection: Arc<tokio::sync::Mutex<()>>,
}
impl Service {
    pub(crate) fn new(db: Database, registry: Registry, ttl: u32) -> Self {
        Self {
            db,
            registry,
            ttl,
            selection: Default::default(),
        }
    }
    fn store(&self) -> Store {
        Store::Sqlite(self.db.clone())
    }
    fn sandboxes(&self) -> ServiceState {
        ServiceState {
            store: self.store(),
            registry: self.registry.clone(),
        }
    }
    async fn dispatch(
        &self,
        binding: &Binding,
        scope: &str,
        operation: Operation,
    ) -> Result<ProviderValue> {
        dispatch_store(
            &self.store(),
            &self.registry,
            binding,
            scope,
            operation,
            Duration::from_secs(600),
            false,
        )
        .await
    }
    async fn models(&self) -> Result<Vec<Value>> {
        let bindings: Vec<Value> = sqlx::query_scalar(
            "SELECT binding FROM provider_connections WHERE status='enabled' ORDER BY provider,id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        let mut models = Vec::new();
        for value in bindings {
            let binding: Binding = serde_json::from_value(value)?;
            if !binding
                .manifest
                .capabilities
                .inference_tasks
                .iter()
                .any(|task| task == "chat")
            {
                continue;
            }
            if binding.manifest.credential.is_some() {
                let reference = magicmtn_engine::provider_credentials::reference(&binding)?;
                if self
                    .registry
                    .admit_credential(&reference, &binding.pin)
                    .is_err()
                {
                    continue;
                }
            }
            for model in &binding.manifest.capabilities.inference_models {
                let hash = Sha256::digest(format!(
                    "{}:{}:{}",
                    binding.connection_id, binding.generation, model.id
                ));
                let id = Uuid::from_bytes(hash[..16].try_into().unwrap());
                models.push(json!({"id":id,"provider":binding.pin.name,"connection_id":binding.connection_id,"source_model_id":model.id,"display_name":model.name,"context_window":model.context_window,"max_tokens":model.max_tokens}));
            }
        }
        Ok(models)
    }
    async fn select(&self, id: Uuid) -> Result<Value> {
        let _lock = self.selection.lock().await;
        let existing: Option<(Value, Value, Option<Value>)> =
            sqlx::query_as("SELECT binding,model,deployment FROM local_models WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.db.pool)
                .await?;
        let (binding, model, mut deployment) = if let Some((binding, model, deployment)) = existing
        {
            (
                serde_json::from_value::<Binding>(binding)?,
                model,
                deployment
                    .map(serde_json::from_value::<InferenceDeployment>)
                    .transpose()?,
            )
        } else {
            let model = self
                .models()
                .await?
                .into_iter()
                .find(|m| m["id"] == id.to_string())
                .context("select an available provider model")?;
            let binding = self
                .db
                .binding(
                    Some(serde_json::from_value(model["connection_id"].clone())?),
                    None,
                    model["provider"].as_str().unwrap(),
                )
                .await?;
            sqlx::query("INSERT INTO local_models(id,binding,model) VALUES(?,?,?)")
                .bind(id.to_string())
                .bind(json!(binding))
                .bind(&model)
                .execute(&self.db.pool)
                .await?;
            (binding, model, None)
        };
        let scope = format!("local-model:{id}");
        if binding
            .manifest
            .capabilities
            .inference_deployment_operations
            .contains(&InferenceDeploymentOperation::Create)
        {
            if deployment.is_none() {
                let value = self
                    .dispatch(
                        &binding,
                        &scope,
                        Operation::InferenceDeploymentCreate {
                            spec: InferenceDeploymentSpec {
                                name: format!("magicmtn-local-{}", id.simple()),
                                model: model["source_model_id"]
                                    .as_str()
                                    .context("model missing")?
                                    .into(),
                                configuration: json!({}),
                            },
                        },
                    )
                    .await?;
                let ProviderValue::InferenceDeployment(created) = value else {
                    bail!("invalid deployment response")
                };
                sqlx::query("UPDATE local_models SET deployment=?,state=? WHERE id=?")
                    .bind(json!(created))
                    .bind(format!("{:?}", created.state))
                    .bind(id.to_string())
                    .execute(&self.db.pool)
                    .await?;
                deployment = Some(created);
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                let resource = deployment.as_ref().unwrap();
                if resource.state == InferenceDeploymentState::Ready {
                    break;
                }
                ensure!(
                    !matches!(
                        resource.state,
                        InferenceDeploymentState::Failed
                            | InferenceDeploymentState::Deleted
                            | InferenceDeploymentState::Deleting
                    ),
                    "model provisioning failed"
                );
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "model is still provisioning; its progress is saved"
                );
                let operation = if resource.state == InferenceDeploymentState::Stopped
                    && binding
                        .manifest
                        .capabilities
                        .inference_deployment_operations
                        .contains(&InferenceDeploymentOperation::Start)
                {
                    Operation::InferenceDeploymentStart {
                        resource: resource.resource.clone(),
                    }
                } else {
                    Operation::InferenceDeploymentStatus {
                        resource: resource.resource.clone(),
                    }
                };
                tokio::time::sleep(Duration::from_secs(1)).await;
                let ProviderValue::InferenceDeployment(current) =
                    self.dispatch(&binding, &scope, operation).await?
                else {
                    bail!("invalid deployment status")
                };
                sqlx::query("UPDATE local_models SET deployment=?,state=? WHERE id=?")
                    .bind(json!(current))
                    .bind(format!("{:?}", current.state))
                    .bind(id.to_string())
                    .execute(&self.db.pool)
                    .await?;
                deployment = Some(current);
            }
        }
        sqlx::query("INSERT INTO local_pi_defaults VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET model_id=excluded.model_id").bind(id.to_string()).execute(&self.db.pool).await?;
        Ok(json!({"saved":true,"model_id":id,"deployment":deployment}))
    }
    async fn session(&self, id: Uuid) -> Result<Value> {
        let row: (String,String,String,i64,Value) = sqlx::query_as("SELECT model_id,provider,desired,generation,snapshot FROM local_pi_sessions WHERE id=?").bind(id.to_string()).fetch_one(&self.db.pool).await?;
        Ok(
            json!({"id":id,"sandbox_id":id,"model_id":row.0,"provider":row.1,"desired":row.2,"generation":row.3,"status":row.4["status"].as_str().unwrap_or("starting"),"exit_code":row.4["exit_code"],"transport":row.4["transport"],"error_category":row.4["error_category"]}),
        )
    }
    async fn exec(&self, id: Uuid, args: Vec<String>) -> Result<String> {
        let record = sandboxes::resolve_store(&self.store(), &id.to_string())
            .await
            .map_err(|(_, e)| anyhow::anyhow!(e))?;
        let binding: Binding =
            serde_json::from_value(record["metadata"]["_magicmtn_connection"].clone())?;
        let sandbox = sandboxes::execution_sandbox(&record).map_err(|(_, e)| anyhow::anyhow!(e))?;
        ensure!(
            args.iter().all(|a| a.len() <= 2048),
            "sandbox transport argument exceeds limit"
        );
        let value = self
            .dispatch(
                &binding,
                record["metadata"]["_magicmtn_scope"]
                    .as_str()
                    .context("sandbox scope missing")?,
                Operation::SandboxExecBounded {
                    sandbox,
                    command: "python3".into(),
                    args,
                    timeout_ms: 10_000,
                    max_output_bytes: 16_384,
                },
            )
            .await?;
        let ProviderValue::Exec(result) = value else {
            bail!("invalid sandbox exec response")
        };
        ensure!(
            !result.timed_out && !result.truncated && result.exit_code == Some(0),
            "sandbox session transport failed"
        );
        Ok(result.stdout)
    }
    async fn upload(&self, id: Uuid, path: &str, data: &[u8]) -> Result<()> {
        ensure!(data.len() <= LIMIT, "session payload too large");
        let temporary = format!("{path}.{}", Uuid::new_v4().simple());
        let program = "import os,base64,sys; p=os.path.expanduser(sys.argv[1]); os.makedirs(os.path.dirname(p),mode=0o700,exist_ok=True); f=os.open(p,os.O_WRONLY|os.O_CREAT,0o600); [os.pwrite(f,base64.b64decode(v),int(sys.argv[2])+i*1200) for i,v in enumerate(sys.argv[3:])]; os.fsync(f); os.close(f)";
        for (index, batch) in data.chunks(1200 * 24).enumerate() {
            let mut args = vec![
                "-c".into(),
                program.into(),
                temporary.clone(),
                (index * 1200 * 24).to_string(),
            ];
            args.extend(batch.chunks(1200).map(|chunk| B64.encode(chunk)));
            self.exec(id, args).await?;
        }
        let digest = hex::encode(Sha256::digest(data));
        self.exec(id, vec!["-c".into(),"import os,sys,hashlib; a,b=map(os.path.expanduser,sys.argv[1:3]); assert hashlib.sha256(open(a,'rb').read()).hexdigest()==sys.argv[3]; os.replace(a,b)".into(),temporary,path.into(),digest]).await?;
        Ok(())
    }
    async fn exchange(&self, id: Uuid, body: Value) -> Result<Value> {
        let encoded = B64.encode(serde_json::to_vec(&body)?);
        Ok(serde_json::from_str(
            &self
                .exec(
                    id,
                    vec![SCRIPT.into(), "exchange".into(), id.to_string(), encoded],
                )
                .await?,
        )?)
    }
    async fn initialize(&self, id: Uuid) -> Result<()> {
        let model: Value = sqlx::query_scalar("SELECT m.model FROM local_models m JOIN local_pi_sessions s ON s.model_id=m.id WHERE s.id=?").bind(id.to_string()).fetch_one(&self.db.pool).await?;
        self.upload(id, SCRIPT, RUNTIME.as_bytes()).await?;
        let config = json!({"session_id":id,"model":{"id":model["id"],"name":model["display_name"],"contextWindow":model["context_window"],"maxTokens":model["max_tokens"],"reasoning":false,"input":["text"],"compat":{"supportsStore":false,"supportsDeveloperRole":false,"maxTokensField":"max_tokens"},"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}});
        self.exec(
            id,
            vec![
                SCRIPT.into(),
                "start".into(),
                id.to_string(),
                B64.encode(serde_json::to_vec(&config)?),
            ],
        )
        .await?;
        sqlx::query("UPDATE local_pi_sessions SET initialized=1 WHERE id=?")
            .bind(id.to_string())
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
    async fn infer(&self, id: Uuid, request_id: &str, bytes: usize) -> Result<Value> {
        ensure!(bytes <= LIMIT, "inference request too large");
        let prior: Option<Option<Value>> = sqlx::query_scalar(
            "SELECT response FROM local_pi_inference WHERE session_id=? AND request_id=?",
        )
        .bind(id.to_string())
        .bind(request_id)
        .fetch_optional(&self.db.pool)
        .await?;
        if let Some(prior) = prior {
            return Ok(prior.unwrap_or_else(|| json!({"error":{"message":"Inference was interrupted; retry the turn in Pi","type":"interrupted"}})));
        }
        let mut data = Vec::new();
        while data.len() < bytes {
            let chunk: Value = serde_json::from_str(
                &self
                    .exec(
                        id,
                        vec![
                            SCRIPT.into(),
                            "read".into(),
                            id.to_string(),
                            request_id.into(),
                            data.len().to_string(),
                        ],
                    )
                    .await?,
            )?;
            let chunk = B64.decode(chunk["data"].as_str().context("missing request chunk")?)?;
            ensure!(
                !chunk.is_empty() && chunk.len() <= 4096,
                "invalid inference chunk"
            );
            data.extend(chunk);
        }
        let mut body: Value = serde_json::from_slice(&data)?;
        let (binding, model, deployment): (Value,Value,Option<Value>) = sqlx::query_as("SELECT m.binding,m.model,m.deployment FROM local_models m JOIN local_pi_sessions s ON s.model_id=m.id WHERE s.id=?").bind(id.to_string()).fetch_one(&self.db.pool).await?;
        let binding: Binding = serde_json::from_value(binding)?;
        ensure!(body["model"] == model["id"], "session model differs");
        let object = body.as_object_mut().context("inference body required")?;
        object.remove("model");
        object.remove("stream");
        object.remove("stream_options");
        for message in body["messages"]
            .as_array_mut()
            .context("messages required")?
        {
            if let Some(parts) = message["content"].as_array() {
                let mut text = String::new();
                for part in parts {
                    ensure!(part["type"] == "text", "this model accepts text input");
                    text.push_str(part["text"].as_str().context("text part required")?);
                }
                message["content"] = json!(text);
            }
        }
        if body.get("store") == Some(&json!(false)) {
            body.as_object_mut().unwrap().remove("store");
        }
        let mut input = magicmtn_sdk::chat::normalize_input(&body)?;
        let mut parameters = input.clone();
        parameters.as_object_mut().unwrap().remove("messages");
        parameters.as_object_mut().unwrap().remove("tools");
        input
            .as_object_mut()
            .unwrap()
            .retain(|key, _| matches!(key.as_str(), "messages" | "tools"));
        let source = deployment
            .as_ref()
            .and_then(|d| d["inference_model"].as_str())
            .or_else(|| model["source_model_id"].as_str())
            .context("serving model missing")?;
        // Claim before invocation. A host crash can fail a turn, never bill it twice.
        let claimed = sqlx::query("INSERT INTO local_pi_inference(session_id,request_id) VALUES(?,?) ON CONFLICT DO NOTHING").bind(id.to_string()).bind(request_id).execute(&self.db.pool).await?.rows_affected();
        ensure!(claimed == 1, "inference already claimed");
        let result = self
            .dispatch(
                &binding,
                &format!("local-model:{}", model["id"].as_str().unwrap()),
                Operation::Infer {
                    request: InferenceRequest {
                        model: ModelConfiguration {
                            model: source.into(),
                            provider: Some(binding.pin.name.clone()),
                            version: None,
                            parameters,
                        },
                        task: "chat".into(),
                        input,
                    },
                },
            )
            .await;
        let response = match result {
            Ok(ProviderValue::Inference(result)) => {
                json!({"id":format!("chatcmpl-{request_id}"),"object":"chat.completion","created":0,"model":model["id"],"choices":[{"index":0,"message":result.output["message"],"finish_reason":result.output["finish_reason"].as_str().unwrap_or("stop")}],"usage":result.output["usage"]})
            }
            _ => {
                tracing::warn!(session_id = %id, operation = "inference", category = "inference_provider", "Local Pi inference provider failed");
                json!({"error":{"message":"The selected inference provider could not complete this turn","type":"provider_error"}})
            }
        };
        sqlx::query("UPDATE local_pi_inference SET response=? WHERE session_id=? AND request_id=?")
            .bind(&response)
            .bind(id.to_string())
            .bind(request_id)
            .execute(&self.db.pool)
            .await?;
        Ok(response)
    }
    async fn tick(&self, id: Uuid) -> Result<()> {
        let (initialized, desired, snapshot): (bool, String, Value) =
            sqlx::query_as("SELECT initialized,desired,snapshot FROM local_pi_sessions WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&self.db.pool)
                .await?;
        if desired == "terminated" {
            return Ok(());
        }
        let sandbox = self
            .db
            .get(id)
            .await?
            .context("sandbox creation still pending")?;
        if sandbox["status"] == "ended" {
            sqlx::query("UPDATE local_pi_sessions SET desired='terminated',snapshot=json_set(snapshot,'$.status','terminated','$.exit_code',0) WHERE id=?").bind(id.to_string()).execute(&self.db.pool).await?;
            return Ok(());
        }
        ensure!(sandbox["status"] == "running", "sandbox is not running");
        if !initialized {
            if desired != "running" {
                return Ok(());
            }
            self.initialize(id).await?;
        }
        let cursor = snapshot["cursor"].as_u64().unwrap_or(0);
        let mut result = self
            .exchange(id, json!({"cursor":cursor,"desired":desired}))
            .await?;
        let inputs: Vec<(String,i64,Value)> = sqlx::query_as("SELECT client_id,seq,body FROM local_pi_inputs WHERE session_id=? ORDER BY rowid LIMIT 8").bind(id.to_string()).fetch_all(&self.db.pool).await?;
        for (client, seq, body) in inputs {
            let data = B64.decode(body["input"].as_str().unwrap_or(""))?;
            for (index, chunk) in data.chunks(900).enumerate() {
                self.exchange(
                    id,
                    json!({"input_id":format!("{client}:{seq}:{index}"),"input":B64.encode(chunk)}),
                )
                .await?;
            }
            self.exchange(id,json!({"input_id":format!("{client}:{seq}:size"),"cols":body["cols"],"rows":body["rows"]})).await?;
            sqlx::query("DELETE FROM local_pi_inputs WHERE session_id=? AND client_id=? AND seq=?")
                .bind(id.to_string())
                .bind(client)
                .bind(seq)
                .execute(&self.db.pool)
                .await?;
        }
        let output = B64.decode(
            result["output"]
                .as_str()
                .context("terminal output missing")?,
        )?;
        let end = result["cursor"]
            .as_i64()
            .context("terminal cursor missing")?;
        let mut tx = self.db.pool.begin().await?;
        if !output.is_empty() {
            sqlx::query("INSERT OR IGNORE INTO local_pi_output VALUES(?,?,?)")
                .bind(id.to_string())
                .bind(end - output.len() as i64)
                .bind(&output)
                .execute(&mut *tx)
                .await?;
        }
        result.as_object_mut().unwrap().remove("output");
        sqlx::query("UPDATE local_pi_sessions SET snapshot=? WHERE id=?")
            .bind(&result)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM local_pi_output WHERE session_id=? AND start<?")
            .bind(id.to_string())
            .bind(end - 1024 * 1024)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn relay(&self, id: Uuid) -> Result<()> {
        let snapshot: Value =
            sqlx::query_scalar("SELECT snapshot FROM local_pi_sessions WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&self.db.pool)
                .await?;
        if let Some(request_id) = snapshot["request"]["id"].as_str() {
            Uuid::parse_str(request_id)?;
            let (response, failure) = match self
                .infer(
                    id,
                    request_id,
                    snapshot["request"]["bytes"]
                        .as_u64()
                        .context("request size missing")? as usize,
                )
                .await
            {
                Ok(response) => (response, None),
                Err(error) => (
                    json!({"error":{"message":"The inference relay could not complete this turn","type":"relay_error"}}),
                    Some(error),
                ),
            };
            let path = format!("~/.magicmtn/local-pi/{id}/{request_id}.response");
            self.upload(id, &path, &serde_json::to_vec(&response)?)
                .await?;
            self.exchange(id, json!({"response_id":request_id})).await?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        Ok(())
    }
    pub(crate) async fn run(self) {
        let mut workers = HashMap::new();
        let mut retry = Retry::new(Duration::from_secs(1));
        loop {
            let result = self.refresh_workers(&mut workers).await;
            tokio::time::sleep(retry.delay(result, None, "session_inventory")).await;
        }
    }

    async fn refresh_workers(&self, workers: &mut HashMap<Uuid, AbortOnDrop>) -> Result<()> {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM local_pi_sessions WHERE desired<>'terminated'")
                .fetch_all(&self.db.pool)
                .await?;
        // Validate the entire inventory before cancelling any existing workers.
        let ids = ids
            .iter()
            .map(|id| id.parse::<Uuid>())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        workers.retain(|id, _| ids.contains(id));
        for id in ids {
            workers.entry(id).or_insert_with(|| self.spawn_worker(id));
        }
        Ok(())
    }

    fn spawn_worker(&self, id: Uuid) -> AbortOnDrop {
        let service = self.clone();
        // Dropping a worker on termination or shutdown cancels both tasks.
        AbortOnDrop(tokio::spawn(async move {
            let relay_service = service.clone();
            let _relay = AbortOnDrop(tokio::spawn(async move {
                let mut retry = Retry::new(Duration::from_millis(250));
                loop {
                    let result = relay_service.relay(id).await;
                    tokio::time::sleep(retry.delay(result, Some(id), "inference_relay")).await;
                }
            }));
            let mut retry = Retry::new(Duration::from_millis(200));
            loop {
                let result = service.tick(id).await;
                if let Err(error) = &result {
                    let category = error_category(error, "sandbox_transport");
                    // Preserve identity and conversation; never replace a sandbox
                    // or agent in response to a transport failure.
                    if let Err(error) = sqlx::query("UPDATE local_pi_sessions SET snapshot=json_set(snapshot,'$.transport','unavailable','$.error_category',?) WHERE id=?")
                        .bind(category).bind(id.to_string()).execute(&service.db.pool).await
                    {
                        tracing::warn!(session_id = %id, operation = "save_transport_status", category = error_category(&error.into(), "database"), "Could not save local Pi transport status");
                    }
                }
                tokio::time::sleep(retry.delay(result, Some(id), "sandbox_transport")).await;
            }
        }))
    }
    pub(crate) fn router(self) -> Router {
        Router::new()
            .route("/v1/pi/catalog", get(catalog))
            .route("/v1/pi/defaults", post(defaults))
            .route("/v1/pi/workspaces", get(list).post(create))
            .route("/v1/pi/workspaces/:id/session", get(status))
            .route("/v1/pi/workspaces/:id/stop", post(stop))
            .route("/v1/pi/workspaces/:id/restart", post(restart))
            .route("/v1/pi/workspaces/:id/terminate", post(terminate))
            .route("/v1/pi/workspaces/:id/terminal", post(attach))
            .route("/v1/pi/workspaces/:id/terminal/poll", post(poll))
            .with_state(self)
    }
}
struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
type Reply = std::result::Result<Json<Value>, (StatusCode, String)>;
fn failure(error: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::CONFLICT, error.to_string())
}
async fn catalog(State(s): State<Service>) -> Reply {
    async {
        let default: Option<String> = sqlx::query_scalar("SELECT model_id FROM local_pi_defaults WHERE singleton=1").fetch_optional(&s.db.pool).await?;
        let images = magicmtn_engine::sandbox_images::list(&s.db).await?;
        Ok(Json(json!({"models":s.models().await?,"defaults":{"model_id":default},"images":images["images"]})))
    }.await.map_err(failure)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    model_id: Uuid,
}
async fn defaults(State(s): State<Service>, Json(body): Json<Selection>) -> Reply {
    s.select(body.model_id).await.map(Json).map_err(failure)
}
async fn list(State(s): State<Service>) -> Reply {
    async {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM local_pi_sessions ORDER BY created_at DESC,id")
                .fetch_all(&s.db.pool)
                .await?;
        let mut sessions = Vec::new();
        for id in ids {
            sessions.push(s.session(id.parse()?).await?);
        }
        Ok(Json(json!({"workspaces":sessions})))
    }
    .await
    .map_err(failure)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct New {
    model_id: Option<Uuid>,
    provider: String,
    idempotency_key: Uuid,
}
async fn create(State(s): State<Service>, Json(body): Json<New>) -> Reply {
    async {
        let existing: Option<(String,String)> = sqlx::query_as("SELECT model_id,provider FROM local_pi_sessions WHERE id=?").bind(body.idempotency_key.to_string()).fetch_optional(&s.db.pool).await?;
        let model = match (existing.as_ref(),body.model_id) {
            (Some((model,_)),None) => model.parse()?,
            (_,Some(model)) => model,
            _ => sqlx::query_scalar::<_,String>("SELECT model_id FROM local_pi_defaults WHERE singleton=1").fetch_optional(&s.db.pool).await?.context("select a default model with /magic default")?.parse()?,
        };
        if let Some((prior,provider)) = &existing { ensure!(*prior == model.to_string() && *provider == body.provider,"creation request differs from its saved selection"); }
        let binding = s.db.binding(None,None,&body.provider).await?;
        ensure!(binding.manifest.capabilities.sandbox_operations.contains(&SandboxOperation::ExecBounded),"sandbox provider needs bounded execution support");
        let resource: Option<(Option<Value>,String)> = sqlx::query_as("SELECT deployment,state FROM local_models WHERE id=?").bind(model.to_string()).fetch_optional(&s.db.pool).await?;
        let (deployment,_) = resource.context("select and provision the model first")?;
        if let Some(deployment) = deployment { ensure!(deployment["state"] == "ready","model is not ready"); }
        sqlx::query("INSERT INTO local_pi_sessions(id,model_id,provider) VALUES(?,?,?) ON CONFLICT DO NOTHING").bind(body.idempotency_key.to_string()).bind(model.to_string()).bind(&body.provider).execute(&s.db.pool).await?;
        sandboxes::create_record(s.sandboxes(),Create { template:String::new(),provider:body.provider,connection_id:Some(binding.connection_id),idempotency_key:Some(body.idempotency_key),notes:Some("Pi".into()) },Some(json!({"kind":"local-pi","session_id":body.idempotency_key,"model_id":model})),&LocalPolicy { ttl_seconds:s.ttl }).await.map_err(|(_,e)| anyhow::anyhow!(e))?;
        s.session(body.idempotency_key).await.map(Json)
    }.await.map_err(failure)
}
async fn status(State(s): State<Service>, Path(id): Path<Uuid>) -> Reply {
    s.session(id).await.map(Json).map_err(failure)
}
async fn stop(State(s): State<Service>, Path(id): Path<Uuid>) -> Reply {
    async {
        sqlx::query(
            "UPDATE local_pi_sessions SET desired='stopped',snapshot=CASE WHEN initialized=0 THEN json_set(snapshot,'$.status','stopped','$.exit_code',0) ELSE snapshot END WHERE id=? AND desired<>'terminated'",
        )
        .bind(id.to_string())
        .execute(&s.db.pool)
        .await?;
        sqlx::query("DELETE FROM local_pi_inputs WHERE session_id=?").bind(id.to_string()).execute(&s.db.pool).await?;
        s.session(id).await.map(Json)
    }
    .await
    .map_err(failure)
}
async fn restart(State(s): State<Service>, Path(id): Path<Uuid>) -> Reply {
    async {
        let session = s.session(id).await?;
        ensure!(session["desired"] != "terminated","session terminated");
        ensure!(matches!(session["status"].as_str(),Some("stopped"|"exited")),"stop the session before restarting");
        sqlx::query("UPDATE local_pi_sessions SET desired='running',initialized=0,generation=generation+1,snapshot=json_set(snapshot,'$.status','starting','$.exit_code',NULL) WHERE id=?").bind(id.to_string()).execute(&s.db.pool).await?;
        s.session(id).await.map(Json)
    }.await.map_err(failure)
}
async fn terminate(State(s): State<Service>, Path(id): Path<Uuid>) -> Reply {
    async {
        s.session(id).await?;
        if s.db.get(id).await?.is_some() {
            sandboxes::terminate_record(s.sandboxes(),&id.to_string()).await.map_err(|(_,e)| anyhow::anyhow!(e))?;
        }
        sqlx::query("UPDATE local_pi_sessions SET desired='terminated',snapshot=json_set(snapshot,'$.status','terminated','$.exit_code',0) WHERE id=?").bind(id.to_string()).execute(&s.db.pool).await?;
        s.session(id).await.map(Json)
    }.await.map_err(failure)
}
#[derive(Deserialize)]
struct Size {
    cols: u16,
    rows: u16,
}
async fn attach(State(s): State<Service>, Path(id): Path<Uuid>, Json(body): Json<Size>) -> Reply {
    async {
        let session = s.session(id).await?;
        ensure!(session["desired"] == "running","restart the session before attaching");
        ensure!(body.cols>0 && body.rows>0,"invalid terminal size");
        let client = Uuid::new_v4();
        sqlx::query("INSERT INTO local_pi_clients(session_id,client_id) VALUES(?,?) ON CONFLICT(session_id) DO UPDATE SET client_id=excluded.client_id,last_seq=-1,cols=0,rows=0").bind(id.to_string()).bind(client.to_string()).execute(&s.db.pool).await?;
        Ok(Json(json!({"client_id":client,"cursor":0})))
    }.await.map_err(failure)
}
#[derive(Deserialize)]
struct Poll {
    client_id: Uuid,
    seq: i64,
    input: String,
    cursor: i64,
    cols: u16,
    rows: u16,
}
async fn poll(State(s): State<Service>, Path(id): Path<Uuid>, Json(body): Json<Poll>) -> Reply {
    async {
        ensure!(body.cursor>=0 && body.seq>=0 && body.cols>0 && body.rows>0,"invalid terminal poll");
        ensure!(body.input.len()<=40_000,"terminal input too large");
        B64.decode(&body.input)?;
        let mut tx=s.db.pool.begin_with("BEGIN IMMEDIATE").await?;
        let (client,last_seq,cols,rows):(String,i64,u16,u16)=sqlx::query_as("SELECT client_id,last_seq,cols,rows FROM local_pi_clients WHERE session_id=?").bind(id.to_string()).fetch_one(&mut *tx).await?;
        ensure!(client==body.client_id.to_string(),"terminal has another attachment");
        if body.seq>last_seq {
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM local_pi_inputs WHERE session_id=?").bind(id.to_string()).fetch_one(&mut *tx).await?;
            ensure!(count<64,"terminal input queue is full");
            if !body.input.is_empty() || body.cols!=cols || body.rows!=rows {
                sqlx::query("INSERT INTO local_pi_inputs VALUES(?,?,?,?)").bind(id.to_string()).bind(&client).bind(body.seq).bind(json!({"input":body.input,"cols":body.cols,"rows":body.rows})).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE local_pi_clients SET last_seq=?,cols=?,rows=? WHERE session_id=?").bind(body.seq).bind(body.cols).bind(body.rows).bind(id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let chunks: Vec<(i64,Vec<u8>)> = sqlx::query_as("SELECT start,data FROM local_pi_output WHERE session_id=? AND start+length(data)>? ORDER BY start LIMIT 4").bind(id.to_string()).bind(body.cursor).fetch_all(&s.db.pool).await?;
        let mut cursor = body.cursor;
        let mut output = Vec::new();
        let mut lost = false;
        for (start,data) in chunks {
            if start>cursor { cursor=start; lost=true; }
            let offset = (cursor-start) as usize;
            output.extend_from_slice(&data[offset..]);
            cursor=start+data.len() as i64;
        }
        let session = s.session(id).await?;
        Ok(Json(json!({"lost":lost,"output":B64.encode(output),"cursor":cursor,"exit_code":session["exit_code"]})))
    }.await.map_err(failure)
}

#[cfg(test)]
#[path = "pi_tests.rs"]
mod tests;
