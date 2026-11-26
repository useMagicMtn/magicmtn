use anyhow::Result;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use magicmtn_engine::{
    provider_connections::{dispatch_store, Binding},
    sandbox_storage::{sqlite::Database, Store},
    sandboxes::execution_sandbox,
    workspace_template::{arguments, Repositories, Specification, CLONE},
};
use magicmtn_provider_framework::host::Registry;
use magicmtn_sdk::plugin::{Operation, Value as ProviderValue};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub fn router(db: Database) -> Router {
    Router::new()
        .route("/templates", get(page))
        .route(
            "/api/templates/default",
            put(save).layer(DefaultBodyLimit::max(32 * 1024)),
        )
        .with_state(db)
}

async fn page(State(db): State<Database>) -> Result<Html<String>, StatusCode> {
    let (description, sqlx::types::Json(github)): (String, sqlx::types::Json<Vec<String>>) =
        sqlx::query_as(
            "SELECT description, github_repos FROM workspace_template WHERE singleton=1",
        )
        .fetch_one(&db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let template = Specification {
        name: "default".into(),
        description,
        repos: Repositories { github },
    };
    magicmtn_web_ui::html(magicmtn_web_ui::TemplatesPage {
        source: template
            .source()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    toml: String,
}

async fn save(State(db): State<Database>, Json(body): Json<Save>) -> Response {
    let spec = match Specification::parse(&body.toml) {
        Ok(spec) => spec,
        Err(error) => return (StatusCode::UNPROCESSABLE_ENTITY, error).into_response(),
    };
    let Ok(source) = spec.source() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    match sqlx::query(
        "UPDATE workspace_template SET description=?, github_repos=? WHERE singleton=1",
    )
    .bind(spec.description)
    .bind(sqlx::types::Json(spec.repos.github))
    .execute(&db.pool)
    .await
    {
        Ok(_) => Json(json!({"toml":source})).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn run(db: Database, registry: Registry) {
    loop {
        match apply_one(&db, &registry).await {
            Ok(true) => continue,
            Ok(false) => (),
            Err(error) => eprintln!("Could not apply workspace template: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn apply_one(db: &Database, registry: &Registry) -> Result<bool> {
    // Claim before dispatch. Never replay a clone after an uncertain result.
    let pending: Option<(String, sqlx::types::Json<Vec<String>>)> = sqlx::query_as(
        "UPDATE template_applications SET claimed=1 WHERE sandbox_id=(
            SELECT a.sandbox_id FROM template_applications a JOIN sandboxes s ON s.id=a.sandbox_id
            WHERE a.claimed=0 AND s.status='running' AND (s.expires_at IS NULL OR unixepoch(s.expires_at)>unixepoch())
            ORDER BY a.rowid LIMIT 1
        ) RETURNING sandbox_id, github_repos"
    ).fetch_optional(&db.pool).await?;
    let Some((id, sqlx::types::Json(repos))) = pending else {
        return Ok(false);
    };
    let Some(record) = db.get(id.parse()?).await? else {
        return Ok(true);
    };
    let sandbox =
        execution_sandbox(&record).map_err(|_| anyhow::anyhow!("invalid sandbox record"))?;
    let binding: Binding =
        serde_json::from_value(record["metadata"]["_magicmtn_connection"].clone())?;
    let scope = format!("sandbox-template:{id}");
    let config: Value = json!({"repos": repos, "origin":"https://github.com", "source":CLONE});
    let op = Operation::SandboxExecBounded {
        sandbox,
        command: "python3".into(),
        args: arguments(&config)?,
        timeout_ms: 10000,
        max_output_bytes: 4096,
    };
    let result = dispatch_store(
        &Store::Sqlite(db.clone()),
        registry,
        &binding,
        &scope,
        op,
        Duration::from_secs(30),
        false,
    )
    .await;
    registry.release(&scope).await;
    anyhow::ensure!(
        matches!(result?, ProviderValue::Exec(output) if output.exit_code==Some(0) && !output.timed_out),
        "workspace template launcher failed for {id}"
    );
    Ok(true)
}
