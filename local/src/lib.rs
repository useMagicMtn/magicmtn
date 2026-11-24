//! Single-user host for the same sandbox API and lifecycle worker used by Cloud.
use anyhow::{ensure, Context, Result};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use magicmtn_engine::sandbox_storage::{sqlite::Database, Store};
use magicmtn_engine::sandboxes::{LocalPolicy, ServiceState};
use magicmtn_provider_framework::host::Registry;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::fs::{self, File};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

mod provider_credentials;
mod shell;

pub static MIGRATOR: LazyLock<sqlx::migrate::Migrator> = LazyLock::new(|| {
    magicmtn_engine::schema::compose(sqlx::migrate!("./migrations").iter().cloned())
});

pub struct Options {
    pub bind: SocketAddr,
    pub directory: PathBuf,
    pub provider_directory: PathBuf,
    pub ttl_seconds: u32,
    pub network: Option<String>,
}

/// Apple Container accepts a network name or `none`.
pub fn network_name(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
        "use a network name containing letters, numbers, periods, underscores, or hyphens"
    );
    Ok(value.to_owned())
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ConnectionInfo {
    pub url: String,
    pub token: String,
}

pub fn directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MAGICMTN_LOCAL_DIRECTORY") {
        return Ok(path.into());
    }
    if let Some(path) = std::env::var_os("MAGICMTN_CONFIG_DIRECTORY") {
        return Ok(PathBuf::from(path).join("local"));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is required")?)
            .join(".config/magicmtn/local"),
    )
}

pub fn connection(directory: &Path) -> Result<ConnectionInfo> {
    let path = directory.join("connection.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            fs::metadata(&path)
                .context("start the local server with `magic server start`")?
                .permissions()
                .mode()
                & 0o077
                == 0,
            "local connection file must be private (0600)"
        );
    }
    serde_json::from_slice(
        &fs::read(path).context("start the local server with `magic server start`")?,
    )
    .context("reading local server connection")
}

/// A connection file survives shutdown; only a held process lock means running.
pub fn is_running(directory: &Path) -> Result<bool> {
    let lock = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("server.lock"))
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("checking local server lock"),
    };
    match lock.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).context("checking local server lock")
        }
    }
}

fn prepare_directory(directory: &Path) -> Result<File> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("server.lock"))?;
    lock.try_lock()
        .context("a local server is already using this directory")?;
    Ok(lock)
}

fn save_connection(directory: &Path, url: String) -> Result<ConnectionInfo> {
    let token = if directory.join("connection.json").exists() {
        connection(directory)?.token
    } else {
        let mut bytes = [0; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| anyhow::anyhow!("generating local credential"))?;
        hex::encode(bytes)
    };
    ensure!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid local credential"
    );
    let info = ConnectionInfo { url, token };
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, &info)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(directory.join("connection.json"))?;
    Ok(info)
}

/// Refuse Cloud and unrelated databases before applying any local migration.
pub async fn initialize(pool: &PgPool) -> Result<()> {
    let (local, occupied): (bool, bool) = sqlx::query_as(
        "SELECT to_regclass('public.local_workspace') IS NOT NULL, EXISTS(SELECT 1 FROM pg_tables WHERE schemaname='public' AND tablename<>'_sqlx_migrations')",
    ).fetch_one(pool).await?;
    ensure!(local || !occupied, "use an empty database dedicated to the local server; this database belongs to another application");
    MIGRATOR.run(pool).await?;
    Ok(())
}

async fn authenticate(
    State(token): State<Arc<String>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // The local API has no browser UI. Reject browser-originated requests as well
    // as requiring a credential, including for reads and health checks.
    if request.headers().contains_key(header::ORIGIN) {
        return Err(StatusCode::FORBIDDEN);
    }
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let expected = token.as_bytes();
    if supplied.len() != expected.len()
        || supplied
            .bytes()
            .zip(expected)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            != 0
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

pub fn router(
    store: impl Into<Store>,
    registry: Registry,
    token: String,
    ttl_seconds: u32,
) -> Router {
    let store = store.into();
    let shell = shell::router(store.clone(), registry.clone());
    magicmtn_engine::sandboxes::router(
        ServiceState { store, registry },
        Arc::new(LocalPolicy { ttl_seconds }),
    )
    .merge(shell)
    .route(
        "/health",
        get(|| async { Json(json!({"status":"ok", "mode":"local"})) }),
    )
    .layer(middleware::from_fn_with_state(
        Arc::new(token),
        authenticate,
    ))
}

pub async fn run(options: Options) -> Result<()> {
    run_with_ready(options, || Ok(())).await
}

pub async fn run_with_ready(options: Options, ready: impl FnOnce() -> Result<()>) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();
    ensure!(
        options.bind.ip().is_loopback(),
        "the local server must bind to loopback"
    );
    ensure!(
        (1..=86400).contains(&options.ttl_seconds),
        "sandbox lifetime must be between 1 and 86400 seconds"
    );
    ensure!(
        std::env::var("MAGICMTN_DEPLOYMENT_MODE").as_deref() != Ok("container"),
        "local startup requires local deployment mode"
    );
    let _lock = prepare_directory(&options.directory)?;
    ensure!(std::env::var_os("MAGICMTN_LOCAL_DATABASE_URL").is_none(), "MAGICMTN_LOCAL_DATABASE_URL is set; choose a fresh local directory for SQLite and remove the database URL setting");
    let path = options.directory.join("magicmtn.sqlite3");
    // A pre-SQLite installation must be selected explicitly by the operator;
    // never replace its apparent state with a fresh empty database.
    ensure!(path.exists() || !options.directory.join("connection.json").exists(), "this directory has an existing server connection but no SQLite database; select a fresh MAGICMTN_LOCAL_DIRECTORY");
    let database = Database::open(&path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    fs::create_dir_all(&options.provider_directory)?;
    let provider_lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.provider_directory.join(".lock"))?;
    provider_lock
        .try_lock_shared()
        .context("provider store is being modified")?;
    let registry = Registry::new(options.provider_directory)?;
    let mut overrides = std::collections::BTreeMap::new();
    if let Some(network) = &options.network {
        overrides.insert(
            "apple-container".into(),
            std::collections::BTreeMap::from([(
                "MAGICMTN_APPLE_CONTAINER_NETWORK".into(),
                network_name(network)?,
            )]),
        );
    }
    database
        .bootstrap_with_configuration(&registry, &overrides)
        .await?;
    let store = Store::Sqlite(database.clone());
    let credentials = provider_credentials::LocalCredentials::open(
        database.clone(),
        registry.clone(),
        &options.directory,
    )
    .await?;
    let listener = tokio::net::TcpListener::bind(options.bind).await?;
    let info = save_connection(
        &options.directory,
        format!("http://{}", listener.local_addr()?),
    )?;
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let credential_routes = credentials.router().layer(middleware::from_fn_with_state(
        Arc::new(info.token.clone()),
        authenticate,
    ));
    let app = router(
        store.clone(),
        registry.clone(),
        info.token.clone(),
        options.ttl_seconds,
    )
    .merge(credential_routes)
    .route(
        "/shutdown",
        post(move || {
            let stop = stop.clone();
            async move {
                let _ = stop.send(true);
                Json(json!({"status":"stopping"}))
            }
        })
        .layer(middleware::from_fn_with_state(
            Arc::new(info.token.clone()),
            authenticate,
        )),
    );
    ready()?;
    let worker = tokio::spawn(magicmtn_engine::sandbox_lifecycle::run_store(
        store.clone(),
        registry.clone(),
    ));
    eprintln!("MagicMtn local server listening on {}", info.url);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! { _ = shutdown() => {}, _ = stopped.changed() => {} }
        })
        .await;
    worker.abort();
    let _ = worker.await;
    registry.shutdown().await;
    database.pool.close().await;
    result.context("serving local sandbox API")
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install shutdown signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = signal.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
