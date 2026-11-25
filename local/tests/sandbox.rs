//! Exercise the local assembly through the shared API, including durable recovery.
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use magicmtn_engine::sandbox_storage::{sqlite::Database, Store};
use magicmtn_local::{initialize, router};
use magicmtn_provider_framework::host::{Registry, Release};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "test-local-token";

fn provider(directory: &std::path::Path) -> anyhow::Result<Registry> {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join("provider");
    let events = directory.join("events");
    let code = format!(
        r#"#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
    request=json.loads(line)
    operation=request['operation']['name']
    with open({events},'a') as output:
        output.write(operation+'\n')
    if operation=='describe':
        result={{'kind':'description','value':{{'manifest_version':1,'provider_version':'0.1.0','capabilities':{{'inference_tasks':[],'sandbox_operations':['create','status','ssh','exec_bounded','terminate'],'editable_surfaces':[],'observe':False,'evaluate':False,'deploy':False}}}}}}
    elif operation=='sandbox_create':
        result={{'kind':'sandbox','value':{{'provider_id':'test-container','status':'running','create_time_ms':1,'metadata':{{}}}}}}
    elif operation=='sandbox_status':
        result={{'kind':'sandbox_status','value':'running'}}
    elif operation=='sandbox_exec_bounded':
        result={{'kind':'exec','value':{{'stdout':'hello\n','stderr':'','exit_code':0,'elapsed_ms':1,'timed_out':False,'truncated':False}}}}
    else:
        result={{'kind':'done'}}
    print(json.dumps({{'id':request['id'],'protocol_version':1,'provider':request['provider'],'outcome':{{'ok':result}}}}),flush=True)
    if operation=='shutdown':
        break
"#,
        events = serde_json::to_string(&events.to_string_lossy())?
    );
    std::fs::write(&script, &code)?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    let release: Release = serde_json::from_value(json!({
        "package": {"name":"test-local", "version":"0.1.0", "protocol_version":1,
            "os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "sha256":hex::encode(ring::digest::digest(&ring::digest::SHA256,code.as_bytes()).as_ref())},
        "manifest":{"manifest_version":1,"provider_version":"0.1.0", "capabilities":{
            "inference_tasks":[],"sandbox_operations":["create","status","ssh","exec_bounded","terminate"],
            "editable_surfaces":[],"observe":false,"evaluate":false,"deploy":false}},
        "active":true,"executable":script,
    }))?;
    Registry::from_releases(vec![release])
}

async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"error":String::from_utf8_lossy(&bytes)}));
    Ok((status, value))
}

#[sqlx::test(migrations = false)]
#[ignore = "requires a disposable PostgreSQL DATABASE_URL"]
async fn local_sandbox_survives_restart_and_uses_durable_cleanup(
    pool: PgPool,
) -> anyhow::Result<()> {
    initialize(&pool).await?;
    exercise_sandbox(pool.into(), None).await
}

#[tokio::test]
async fn sqlite_sandbox_survives_restart_and_uses_durable_cleanup() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("magicmtn.sqlite3");
    let db = Database::open(&path).await?;
    let private_tables:i64=sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name IN ('users','organizations','organization_members','billing_accounts')").fetch_one(&db.pool).await?;
    assert_eq!(private_tables, 0);
    exercise_sandbox(Store::Sqlite(db), Some(&path)).await
}

async fn bootstrap(store: &Store, registry: &Registry) -> anyhow::Result<()> {
    match store {
        Store::Postgres(pool) => {
            magicmtn_engine::provider_connections::bootstrap_local(pool, registry).await?;
            magicmtn_engine::provider_connections::validate_catalog_references(pool, registry)
                .await?;
        }
        Store::Sqlite(db) => db.bootstrap(registry).await?,
    }
    Ok(())
}

async fn exercise_sandbox(mut store: Store, path: Option<&std::path::Path>) -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let registry = provider(directory.path())?;
    bootstrap(&store, &registry).await?;
    let app = router(store.clone(), registry.clone(), TOKEN.into(), 3600);
    let id = Uuid::new_v4();
    let create =
        json!({"template":"image:alpine:3.24.1","provider":"test-local","idempotency_key":id});
    let (status, saved) = request(&app, "POST", "/v1/sandboxes", create.clone()).await?;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["status"], "running");
    let (status, shell) = request(
        &app,
        "POST",
        &format!("/v1/sandboxes/{id}/shell"),
        json!(null),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{shell}");
    assert_eq!(shell["pin"], saved["metadata"]["_magicmtn_provider"]);
    assert_eq!(shell["sandbox"]["provider_id"], "test-container");
    assert_eq!(shell["configuration"], json!({}));
    assert_eq!(
        request(&app, "POST", "/v1/sandboxes", create).await?.0,
        StatusCode::OK
    );
    let conflict = json!({"template":"image:other","provider":"test-local","idempotency_key":id});
    assert_eq!(
        request(&app, "POST", "/v1/sandboxes", conflict).await?.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("events"))?
            .lines()
            .filter(|line| *line == "sandbox_create")
            .count(),
        1
    );
    let (_, result) = request(
        &app,
        "POST",
        &format!("/v1/sandboxes/{id}/exec"),
        json!({"command":"echo","args":["hello"],"timeout_ms":1000,"max_output_bytes":1024}),
    )
    .await?;
    assert_eq!(result["stdout"], "hello\n", "{result}");
    registry.shutdown().await;
    drop(app);
    // Reconstruct all process state and reapply startup against durable records.
    match &store {
        Store::Postgres(pool) => initialize(pool).await?,
        Store::Sqlite(db) => {
            db.pool.close().await;
            store = Store::Sqlite(Database::open(path.unwrap()).await?);
        }
    }
    let registry = provider(directory.path())?;
    bootstrap(&store, &registry).await?;
    let app = router(store.clone(), registry.clone(), TOKEN.into(), 3600);
    assert_eq!(
        request(&app, "GET", &format!("/v1/sandboxes/{id}"), json!(null))
            .await?
            .1["provider_id"],
        "test-container"
    );
    store.schedule().await?;
    let (status, ended) = request(
        &app,
        "POST",
        &format!("/v1/sandboxes/{id}/terminate"),
        json!({}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert_eq!(ended["status"], "ended", "{ended}");
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/sandboxes/{id}/shell"),
            json!(null)
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/sandboxes/{id}/exec"),
            json!({"command":"echo","timeout_ms":1000,"max_output_bytes":1024})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/sandboxes/{id}/terminate"),
            json!({})
        )
        .await?
        .1["status"],
        "ended"
    );
    registry.shutdown().await;
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires a disposable PostgreSQL DATABASE_URL"]
async fn initialization_refuses_an_unrelated_database(pool: PgPool) -> anyhow::Result<()> {
    sqlx::query("CREATE TABLE users(id integer)")
        .execute(&pool)
        .await?;
    assert!(initialize(&pool).await.is_err());
    let migrated: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&pool)
            .await?;
    assert!(!migrated);
    Ok(())
}

#[tokio::test]
async fn local_api_requires_a_credential_and_rejects_browser_origins() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let app = router(
        PgPool::connect_lazy("postgresql://localhost/unused")?,
        Registry::new(directory.path().into())?,
        TOKEN.into(),
        3600,
    );
    for (token, origin, expected) in [
        (None, None, StatusCode::UNAUTHORIZED),
        (Some("wrong"), None, StatusCode::UNAUTHORIZED),
        (
            Some(TOKEN),
            Some("https://example.com"),
            StatusCode::FORBIDDEN,
        ),
        (Some(TOKEN), None, StatusCode::OK),
    ] {
        let mut request = Request::builder().uri("/health");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty())?)
                .await?
                .status(),
            expected
        );
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
#[ignore = "requires a disposable PostgreSQL DATABASE_URL"]
async fn host_policy_refusal_never_dispatches_a_provider(pool: PgPool) -> anyhow::Result<()> {
    use magicmtn_engine::sandboxes::{ApiError, PolicyFuture, SandboxPolicy, SandboxState};
    struct Refuse {
        before_intent: bool,
    }
    impl SandboxPolicy for Refuse {
        fn ttl_seconds(&self, _provider: &str) -> Result<u32, ApiError> {
            Ok(3600)
        }
        fn preferred_connection(&self, provider: &str) -> Option<String> {
            Some(format!("connection:self-{provider}"))
        }
        fn admit<'a>(
            &'a self,
            _: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
            _: Option<Uuid>,
        ) -> PolicyFuture<'a> {
            Box::pin(async move {
                if self.before_intent {
                    Err((StatusCode::TOO_MANY_REQUESTS, "admission refused".into()))
                } else {
                    Ok(())
                }
            })
        }
        fn authorize<'a>(
            &'a self,
            _: &'a PgPool,
            _: Option<Uuid>,
            _: &'a magicmtn_engine::provider_connections::Binding,
            _: Uuid,
            _: u32,
            _: &'a str,
        ) -> PolicyFuture<'a> {
            Box::pin(async { Err((StatusCode::PAYMENT_REQUIRED, "spending refused".into())) })
        }
    }
    initialize(&pool).await?;
    let directory = tempfile::tempdir()?;
    let registry = provider(directory.path())?;
    magicmtn_engine::provider_connections::bootstrap_local(&pool, &registry).await?;
    for before_intent in [true, false] {
        let app = magicmtn_engine::sandboxes::router(
            SandboxState {
                pool: pool.clone(),
                registry: registry.clone(),
            },
            std::sync::Arc::new(Refuse { before_intent }),
        );
        let id = Uuid::new_v4();
        let (status, _) = request(
            &app,
            "POST",
            "/v1/sandboxes",
            json!({"template":"image:alpine:3.24.1","provider":"test-local","idempotency_key":id}),
        )
        .await?;
        assert_eq!(
            status,
            if before_intent {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::PAYMENT_REQUIRED
            }
        );
        let saved: Option<String> = sqlx::query_scalar("SELECT status FROM sandboxes WHERE id=$1")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        assert_eq!(
            saved,
            if before_intent {
                None
            } else {
                Some("failed".into())
            }
        );
        assert!(!directory.path().join("events").exists());
    }
    registry.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn sqlite_recovers_received_create_and_expires_without_replaying() -> anyhow::Result<()> {
    use magicmtn_engine::sandboxes::{Create, LocalPolicy};
    use magicmtn_engine::{provider_connections, sandbox_lifecycle};
    use magicmtn_sdk::plugin::Operation;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.sqlite3");
    let db = Database::open(&path).await?;
    let registry = provider(directory.path())?;
    db.bootstrap(&registry).await?;
    let store = Store::Sqlite(db.clone());
    let binding = store
        .binding(None, Some("connection:self-test-local"), "test-local")
        .await?;
    let id = Uuid::new_v4();
    let scope = format!("sandbox:{id}");
    let body: Create = serde_json::from_value(
        json!({"provider":"test-local","template":"image:alpine","idempotency_key":id}),
    )?;
    let metadata = json!({"_magicmtn_connection":binding,"_magicmtn_provider":binding.pin,"_magicmtn_scope":scope});
    store
        .reserve(
            id,
            &body,
            &metadata,
            "_magicmtn_workspace",
            None,
            1,
            &LocalPolicy { ttl_seconds: 1 },
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    provider_connections::dispatch_store(
        &store,
        &registry,
        &binding,
        &scope,
        Operation::SandboxCreate {
            template: format!("image:alpine|{id}"),
            ttl_seconds: Some(1),
        },
        std::time::Duration::from_secs(5),
        false,
    )
    .await?;
    // Crash after the provider receipt commits but before attachment. Cancellation
    // is also durable and must win when recovery attaches the native identity.
    store.request_termination(id, "sandbox_ttl_expired").await?;
    registry.shutdown().await;
    db.pool.close().await;
    drop(store);
    let db = Database::open(&path).await?;
    db.bootstrap(&registry).await?;
    let store = Store::Sqlite(db.clone());
    store.schedule().await?;
    let saved = store.get(id).await?.unwrap();
    assert_eq!(saved["provider_id"], "test-container");
    assert_eq!(saved["status"], "terminating");
    assert!(sandbox_lifecycle::run_one_store(&store, &registry, Some(id)).await?);
    let saved = store.get(id).await?.unwrap();
    assert_eq!(saved["status"], "ended");
    assert_eq!(saved["shutdown_reason"], "sandbox_ttl_expired");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("events"))?
            .lines()
            .filter(|s| *s == "sandbox_create")
            .count(),
        1
    );

    // A reserved but unacknowledged create can never be automatically replayed.
    let lost_scope = format!("sandbox:{}", Uuid::new_v4());
    assert!(
        store
            .reserve_receipt(&binding, &lost_scope, "sandbox")
            .await?
    );
    assert!(provider_connections::dispatch_store(
        &store,
        &registry,
        &binding,
        &lost_scope,
        Operation::SandboxCreate {
            template: "image:alpine".into(),
            ttl_seconds: Some(1)
        },
        std::time::Duration::from_secs(5),
        false
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("events"))?
            .lines()
            .filter(|s| *s == "sandbox_create")
            .count(),
        1
    );
    registry.shutdown().await;
    db.pool.close().await;
    Ok(())
}

#[tokio::test]
async fn sqlite_expiration_rejects_execution_and_schedules_cleanup() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let db = Database::open(&directory.path().join("state.sqlite3")).await?;
    let registry = provider(directory.path())?;
    db.bootstrap(&registry).await?;
    let store = Store::Sqlite(db.clone());
    let app = router(store.clone(), registry.clone(), TOKEN.into(), 1);
    let id = Uuid::new_v4();
    let (status, saved) = request(
        &app,
        "POST",
        "/v1/sandboxes",
        json!({"provider":"test-local","template":"image:alpine","idempotency_key":id}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{saved}");
    sqlx::query("UPDATE sandboxes SET record=json_set(record,'$.expires_at','2000-01-01T00:00:00Z') WHERE id=?").bind(id.to_string()).execute(&db.pool).await?;
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/sandboxes/{id}/exec"),
            json!({"command":"echo","timeout_ms":1000,"max_output_bytes":1024})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/sandboxes/{id}/shell"),
            json!(null)
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    store.schedule().await?;
    magicmtn_engine::sandbox_lifecycle::run_one_store(&store, &registry, Some(id)).await?;
    let saved = store.get(id).await?.unwrap();
    assert_eq!(saved["status"], "ended");
    assert_eq!(saved["shutdown_reason"], "sandbox_ttl_expired");
    registry.shutdown().await;
    db.pool.close().await;
    Ok(())
}

#[tokio::test]
async fn sqlite_refuses_unrelated_databases() -> anyhow::Result<()> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("unrelated.sqlite3");
    let pool = SqlitePoolOptions::new()
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await?;
    sqlx::query("CREATE TABLE users(id INTEGER)")
        .execute(&pool)
        .await?;
    assert!(Database::open(&path).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name='_sqlx_migrations'"
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn local_shell_requires_authentication_and_an_enabled_connection() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let db = Database::open(&directory.path().join("state.sqlite3")).await?;
    let registry = provider(directory.path())?;
    db.bootstrap(&registry).await?;
    let app = router(
        Store::Sqlite(db.clone()),
        registry.clone(),
        TOKEN.into(),
        3600,
    );
    let id = Uuid::new_v4();
    let (_, saved) = request(
        &app,
        "POST",
        "/v1/sandboxes",
        json!({"provider":"test-local","template":"image:alpine","idempotency_key":id}),
    )
    .await?;
    let path = format!("/v1/sandboxes/{id}/shell");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE provider_connections SET status='disabled'")
        .execute(&db.pool)
        .await?;
    assert_eq!(
        request(&app, "POST", &path, json!(null)).await?.0,
        StatusCode::CONFLICT
    );
    // Restart configuration updates future sandboxes without rewriting existing snapshots.
    sqlx::query(
        "UPDATE provider_connections SET binding=json_set(binding,'$.configuration',json(?))",
    )
    .bind(r#"{"MAGICMTN_APPLE_CONTAINER_NETWORK":"none"}"#)
    .execute(&db.pool)
    .await?;
    db.bootstrap(&registry).await?;
    let binding: Value = sqlx::query_scalar("SELECT binding FROM provider_connections")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(binding["configuration"], json!({}));
    assert_eq!(binding["generation"], 2);
    assert_eq!(
        binding["connection_id"],
        saved["metadata"]["_magicmtn_connection"]["connection_id"]
    );
    assert_eq!(db.get(id).await?.unwrap()["metadata"], saved["metadata"]);
    db.bootstrap(&registry).await?;
    let generation: i64 =
        sqlx::query_scalar("SELECT json_extract(binding,'$.generation') FROM provider_connections")
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(generation, 2);
    registry.shutdown().await;
    db.pool.close().await;
    Ok(())
}

#[tokio::test]
async fn explicit_configuration_is_frozen_per_sandbox() -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    let directory = tempfile::tempdir()?;
    let db = Database::open(&directory.path().join("state.sqlite3")).await?;
    let registry = provider(directory.path())?;
    let overrides = |network: &str| {
        BTreeMap::from([(
            "test-local".into(),
            BTreeMap::from([("MAGICMTN_APPLE_CONTAINER_NETWORK".into(), network.into())]),
        )])
    };
    db.bootstrap_with_configuration(&registry, &overrides("default"))
        .await?;
    let app = router(
        Store::Sqlite(db.clone()),
        registry.clone(),
        TOKEN.into(),
        3600,
    );
    let (status, saved) = request(
        &app,
        "POST",
        "/v1/sandboxes",
        json!({"provider":"test-local","template":"image:alpine"}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["metadata"]["_magicmtn_connection"]["configuration"]
            ["MAGICMTN_APPLE_CONTAINER_NETWORK"],
        "default"
    );
    db.bootstrap_with_configuration(&registry, &overrides("none"))
        .await?;
    let binding = db.binding(None, None, "test-local").await?;
    assert_eq!(
        binding.configuration["MAGICMTN_APPLE_CONTAINER_NETWORK"],
        "none"
    );
    let id = Uuid::parse_str(saved["id"].as_str().unwrap())?;
    assert_eq!(db.get(id).await?.unwrap()["metadata"], saved["metadata"]);
    registry.shutdown().await;
    db.pool.close().await;
    Ok(())
}

#[tokio::test]
async fn canonical_default_is_frozen_before_dispatch_and_survives_restart() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let registry = provider(directory.path())?;
    let path = directory.path().join("canonical.sqlite3");
    let database = Database::open(&path).await?;
    database.bootstrap(&registry).await?;
    magicmtn_engine::sandbox_images::seed(&database).await?;
    let binding = database.binding(None, None, "test-local").await?;
    let platform = magicmtn_sdk::image_registry::local_platform()?;
    magicmtn_engine::sandbox_images::save(
        &database,
        "canonical",
        &binding,
        &magicmtn_sdk::image_registry::PreparedImage {
            image: magicmtn_sdk::image_registry::canonical(platform)?,
            platform: platform.into(),
            artifact: "prepared-fixture".into(),
            configuration: Default::default(),
        },
    )
    .await?;
    let app = router(
        Store::Sqlite(database.clone()),
        registry.clone(),
        TOKEN.into(),
        3600,
    );
    let id = Uuid::new_v4();
    let body = json!({"provider": "test-local", "idempotency_key": id});
    let (status, record) = request(&app, "POST", "/v1/sandboxes", body.clone()).await?;
    assert_eq!(status, StatusCode::OK, "{record}");
    let platform = magicmtn_sdk::image_registry::local_platform()?;
    let expected = format!(
        "image:{}",
        magicmtn_sdk::image_registry::canonical(platform)?.reference()
    );
    assert_eq!(record["template"], expected);
    drop(app);
    database.pool.close().await;
    let database = Database::open(&path).await?;
    database.bootstrap(&registry).await?;
    let app = router(
        Store::Sqlite(database),
        registry.clone(),
        TOKEN.into(),
        3600,
    );
    let (status, replayed) = request(&app, "POST", "/v1/sandboxes", body).await?;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["template"], expected);
    assert_eq!(replayed["id"], record["id"]);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("events"))?
            .lines()
            .filter(|line| *line == "sandbox_create")
            .count(),
        1
    );
    registry.shutdown().await;
    Ok(())
}
