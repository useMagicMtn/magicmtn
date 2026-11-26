use anyhow::{ensure, Context, Result};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path as RoutePath, State},
    http::StatusCode,
    routing::{post, put},
    Json, Router,
};
use magicmtn_engine::{
    provider_connections::Binding,
    provider_credentials::{parse_bundle, reference, MAX_BUNDLE_BYTES},
    sandbox_storage::sqlite::Database,
};
use magicmtn_provider_framework::host::{PreparedCredential, ProcessEnvironment, Registry};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::json;
use std::{fs, io::Write, path::Path, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct LocalCredentials {
    database: Database,
    registry: Registry,
    key: Arc<aead::LessSafeKey>,
    writes: Arc<Mutex<()>>,
    state_directory: std::path::PathBuf,
}

#[derive(sqlx::FromRow)]
struct Stored {
    connection_id: String,
    provider: String,
    account: String,
    version: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl Stored {
    fn aad(&self, installation: Uuid) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&(
            installation,
            &self.connection_id,
            &self.provider,
            &self.account,
            &self.version,
        ))?)
    }
}

impl LocalCredentials {
    pub(crate) async fn open(
        database: Database,
        registry: Registry,
        directory: &Path,
    ) -> Result<Self> {
        let rows: Vec<Stored> = sqlx::query_as("SELECT connection_id, provider, account, version, nonce, ciphertext FROM provider_credentials")
            .fetch_all(&database.pool).await?;
        let key = load_key(directory, rows.is_empty())?;
        let service = Self {
            database,
            registry,
            key: Arc::new(key),
            writes: Default::default(),
            state_directory: directory.canonicalize()?.join("provider-state"),
        };
        for mut row in rows {
            let id = Uuid::parse_str(&row.connection_id)?;
            let binding = service
                .database
                .binding(Some(id), None, &row.provider)
                .await?;
            ensure!(
                binding.pin.name == row.provider,
                "stored provider credential identity mismatch"
            );
            let aad = row.aad(service.database.id)?;
            let nonce = aead::Nonce::try_assume_unique_for_key(&row.nonce)
                .map_err(|_| anyhow::anyhow!("invalid credential nonce"))?;
            let mut ciphertext = zeroize::Zeroizing::new(std::mem::take(&mut row.ciphertext));
            let values = parse_bundle(
                service
                    .key
                    .open_in_place(nonce, aead::Aad::from(aad), &mut ciphertext)
                    .map_err(|_| {
                        anyhow::anyhow!("stored provider credential failed authentication")
                    })?,
            )?;
            let update = service
                .registry
                .begin_credential_update(reference(&binding)?)?;
            service.registry.publish_credential(
                update,
                PreparedCredential {
                    version: row.version,
                    account: row.account,
                    values: Arc::new(values),
                    expires_at: None,
                },
            )?;
        }
        Ok(service)
    }

    pub(crate) fn router(self) -> Router {
        Router::new()
            .route("/v1/providers/:name/credential", put(configure))
            .route("/v1/providers/:name/configure", put(setup))
            .route("/v1/providers/:name/verify", post(verify))
            .layer(DefaultBodyLimit::max(MAX_BUNDLE_BYTES))
            .with_state(self)
    }

    async fn binding(&self, name: &str) -> Result<Binding> {
        let installed = self.registry.registry_release(name)?;
        let binding = self.database.binding(None, None, name).await?;
        ensure!(
            binding.pin == installed.pin(),
            "provider release differs from the active connection"
        );
        ensure!(
            binding.manifest.credential.is_some(),
            "provider uses workload identity or has no credential fields"
        );
        Ok(binding)
    }

    async fn save(
        &self,
        mut binding: Binding,
        body: &[u8],
        options: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Result<()> {
        let values = parse_bundle(body)?;
        binding
            .manifest
            .credential
            .as_ref()
            .context("provider credential declaration missing")?
            .check_values(&values)?;
        let key = reference(&binding)?;
        let update = self.registry.begin_credential_update(key)?;
        let environment = ProcessEnvironment {
            values: binding.configuration.clone(),
        };
        let mut setup_configuration = None;
        let account = if let Some(options) = options {
            let mut options = options.clone();
            if binding
                .manifest
                .capabilities
                .setup
                .as_ref()
                .is_some_and(|setup| setup.options.contains_key("state_directory"))
            {
                options.entry("state_directory".into()).or_insert_with(|| {
                    self.state_directory
                        .join(binding.connection_id.to_string())
                        .to_string_lossy()
                        .into_owned()
                });
            }
            let connection = self
                .registry
                .configure_credential(&binding.pin, &environment, &values, &options)
                .await?;
            setup_configuration = Some(connection.configuration.clone());
            binding.configuration.extend(connection.configuration);
            connection.account
        } else {
            self.registry
                .verify_credential(&binding.pin, &environment, &values)
                .await?
        };
        let previous_account: Option<String> =
            sqlx::query_scalar("SELECT account FROM provider_credentials WHERE connection_id=?")
                .bind(binding.connection_id.to_string())
                .fetch_optional(&self.database.pool)
                .await?;
        ensure!(
            previous_account
                .as_ref()
                .is_none_or(|previous| previous == &account),
            "provider account differs from the connection"
        );
        let random = SystemRandom::new();
        let mut nonce = [0; 12];
        random
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("cannot generate credential nonce"))?;
        let mut row = Stored {
            connection_id: binding.connection_id.to_string(),
            provider: binding.pin.name.clone(),
            account,
            version: Uuid::new_v4().to_string(),
            nonce: nonce.to_vec(),
            ciphertext: body.to_vec(),
        };
        let aad = row.aad(self.database.id)?;
        let mut ciphertext = zeroize::Zeroizing::new(std::mem::take(&mut row.ciphertext));
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad),
                &mut *ciphertext,
            )
            .map_err(|_| anyhow::anyhow!("cannot encrypt provider credential"))?;
        let mut transaction = self.database.pool.begin().await?;
        let changed = sqlx::query(
            "INSERT INTO provider_credentials(connection_id,provider,account,version,nonce,ciphertext)
             VALUES(?,?,?,?,?,?) ON CONFLICT(connection_id) DO UPDATE SET
             version=excluded.version, nonce=excluded.nonce, ciphertext=excluded.ciphertext
             WHERE account=excluded.account AND provider=excluded.provider"
        ).bind(&row.connection_id).bind(&row.provider).bind(&row.account).bind(&row.version)
            .bind(&row.nonce).bind(ciphertext.as_slice()).execute(&mut *transaction).await?.rows_affected();
        ensure!(changed == 1, "provider account differs from the connection");
        if let Some(setup_configuration) = setup_configuration {
            sqlx::query("INSERT INTO provider_setup(connection_id,configuration) VALUES(?,?) ON CONFLICT(connection_id) DO UPDATE SET configuration=excluded.configuration")
                .bind(&row.connection_id).bind(serde_json::to_string(&setup_configuration)?)
                .execute(&mut *transaction).await?;
            // Advance only when setup changes the binding; repeated configuration
            // keeps prepared images reusable. Frozen sandbox bindings stay intact.
            sqlx::query("UPDATE provider_connections SET binding=json_set(?, '$.generation', json_extract(binding,'$.generation')+1) WHERE id=? AND json_extract(binding,'$.configuration')<>json_extract(?,'$.configuration')")
                .bind(serde_json::to_string(&binding)?).bind(&row.connection_id).bind(serde_json::to_string(&binding)?)
                .execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        self.registry.publish_credential(
            update,
            PreparedCredential {
                version: row.version,
                account: row.account,
                values: Arc::new(values),
                expires_at: None,
            },
        )
    }
}

async fn configure(
    State(service): State<LocalCredentials>,
    RoutePath(name): RoutePath<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let result = tokio::spawn(async move {
        let _write = service.writes.lock().await;
        let binding = service
            .binding(&name)
            .await
            .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
        service.save(binding, &body, None).await.map_err(|error| {
            // The verification call suppresses vendor diagnostics. Keep storage and
            // transport errors private as well.
            let message = error.to_string();
            let message = if message.starts_with("provider credential requires ")
                || matches!(
                    message.as_str(),
                    "unexpected provider credential fields"
                        | "provider account differs from the connection"
                        | "provider credential verification failed"
                        | "expected a JSON object with unique string fields"
                        | "expected a single JSON credential object"
                ) {
                message
            } else {
                "unable to configure provider credential".into()
            };
            (StatusCode::BAD_REQUEST, message)
        })?;
        Ok(Json(json!({"provider": name, "status": "ready"})))
    })
    .await;
    result.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider configuration failed".into(),
        )
    })?
}

async fn verify(
    State(service): State<LocalCredentials>,
    RoutePath(name): RoutePath<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let binding = service.binding(&name).await.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "provider connection unavailable".into(),
        )
    })?;
    let key = reference(&binding).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "provider connection unavailable".into(),
        )
    })?;
    service
        .registry
        .verify_connection(
            &key,
            &binding.pin,
            &ProcessEnvironment {
                values: binding.configuration,
            },
        )
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "provider connection verification failed".into(),
            )
        })?;
    Ok(Json(json!({"provider":name,"status":"ready"})))
}

/// Explicit setup accepts nonsecret provider options beside the credential bundle.
async fn setup(
    State(service): State<LocalCredentials>,
    RoutePath(name): RoutePath<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        credential: magicmtn_sdk::plugin::CredentialValues,
        #[serde(default)]
        options: std::collections::BTreeMap<String, String>,
    }
    let input: Input = serde_json::from_slice(&body).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "invalid provider configuration".into(),
        )
    })?;
    let credential =
        zeroize::Zeroizing::new(serde_json::to_vec(&input.credential).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "invalid provider credential".into(),
            )
        })?);
    tokio::spawn(async move {
        let _write = service.writes.lock().await;
        let binding = service.binding(&name).await.map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "provider connection unavailable".into(),
            )
        })?;
        service
            .save(binding, &credential, Some(&input.options))
            .await
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "provider setup failed; check credentials and setup options".into(),
                )
            })?;
        Ok(Json(json!({"provider":name,"status":"ready"})))
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "provider configuration failed".into(),
        )
    })?
}

fn load_key(directory: &Path, create: bool) -> Result<aead::LessSafeKey> {
    let path = directory.join("provider-credentials.key");
    if !path.exists() {
        ensure!(
            create,
            "restore the provider credential key for this installation"
        );
        let mut bytes = zeroize::Zeroizing::new([0; 32]);
        SystemRandom::new()
            .fill(&mut *bytes)
            .map_err(|_| anyhow::anyhow!("cannot generate provider credential key"))?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(&*bytes)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&path)?;
        #[cfg(unix)]
        fs::File::open(directory)?.sync_all()?;
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() == 32,
        "invalid provider credential key file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "provider credential key file must be private"
        );
    }
    let bytes = zeroize::Zeroizing::new(fs::read(path)?);
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &bytes)
        .map_err(|_| anyhow::anyhow!("invalid provider credential key"))?;
    Ok(aead::LessSafeKey::new(key))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use magicmtn_sdk::plugin::{Capabilities, CredentialSpec, ProviderManifest, SandboxOperation};
    use ring::digest::{digest, SHA256};
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn configuration_is_bound_to_account_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let providers = root.join("providers");
        let registry = Registry::new(providers.clone()).unwrap();
        let release = providers.join("fixture/1.0.0");
        fs::create_dir_all(&release).unwrap();
        let manifest = ProviderManifest {
            manifest_version: 1,
            provider_version: "1.0.0".into(),
            credential: Some(CredentialSpec {
                fields: vec!["TOKEN".into()],
            }),
            capabilities: {
                let mut capabilities = Capabilities::sandbox(vec![
                    SandboxOperation::Create,
                    SandboxOperation::ExecBounded,
                    SandboxOperation::Terminate,
                ]);
                capabilities.setup = Some(magicmtn_sdk::provider_setup::SetupSpec {
                    options: [(
                        "state_directory".into(),
                        magicmtn_sdk::provider_setup::ConfigurationOption {
                            description: "Persistent provider state".into(),
                            default: None,
                        },
                    )]
                    .into(),
                });
                capabilities
            },
        };
        let script = r#"#!/bin/sh
test -z "${TOKEN+x}" || exit 1
IFS= read -r frame || exit 1
case "$frame" in
  *account-a-v1*) account=a; version=first ;;
  *account-a-v2*) account=a; version=second ;;
  *account-b-v1*) account=b; version=other ;;
  *) exit 1 ;;
esac
reply() {
    printf '{"id":%s,"protocol_version":2,"provider":"fixture","outcome":{"ok":%s}}\n' "$1" "$2"
}
case "$frame" in
  *'"verify_account":true'*) reply 1 '{"kind":"verified_account","value":"'"$account"'"}'; exit ;;
esac
reply 1 '__MANIFEST__'
id=1
while IFS= read -r frame; do
    id=$((id+1))
    case "$frame" in
      *'"name":"provider_configure"'*)
        case "$frame" in *'"state_directory":"__STATE_ROOT__/'*) ;; *) exit 1 ;; esac
        printf 'setup\n' >> '__SETUP_LOG__'; reply "$id" '{"kind":"configured_connection","value":{"account":"'"$account"'","configuration":{"MAGICMTN_FIXTURE_APP_ID":"app-a"}}}' ;;
      *'"name":"sandbox_create"'*) reply "$id" '{"kind":"sandbox","value":{"provider_id":"account-a-sandbox","status":"running","create_time_ms":null,"metadata":null}}' ;;
      *'"name":"sandbox_exec_bounded"'*) reply "$id" '{"kind":"exec","value":{"stdout":"'"$version"'","stderr":"","exit_code":0,"elapsed_ms":0,"timed_out":false,"truncated":false}}' ;;
      *) exit 1 ;;
    esac
done
"#.replace("__MANIFEST__", &json!({"kind":"description", "value":manifest}).to_string()).replace("__SETUP_LOG__", root.join("setup-calls").to_str().unwrap()).replace("__STATE_ROOT__", root.canonicalize().unwrap().join("provider-state").to_str().unwrap());
        let digest = hex::encode(digest(&SHA256, script.as_bytes()));
        let package = json!({"name":"fixture","version":"1.0.0","protocol_version":2,
            "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"sha256":digest});
        fs::write(release.join("provider"), script).unwrap();
        fs::set_permissions(release.join("provider"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(release.join("package.json"), package.to_string()).unwrap();
        fs::write(
            release.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(providers.join("fixture/active"), "1.0.0").unwrap();
        fs::write(release.join("registry-origin.json"), json!({
            "registry":"registry-fixture", "pin":{"name":"fixture","version":"1.0.0","sha256":digest},
            "archive_sha256":"a".repeat(64)
        }).to_string()).unwrap();
        let database = Database::open(&root.join("local.sqlite")).await.unwrap();
        database.bootstrap(&registry).await.unwrap();
        let build_router =
            |registry: Registry, credentials: LocalCredentials| {
                super::super::router(
                    magicmtn_engine::sandbox_storage::Store::Sqlite(database.clone()),
                    registry,
                    "fixture-auth".into(),
                    60,
                )
                .merge(credentials.router().layer(
                    axum::middleware::from_fn_with_state(
                        Arc::new("fixture-auth".to_owned()),
                        super::super::authenticate,
                    ),
                ))
            };
        let credentials = LocalCredentials::open(database.clone(), registry.clone(), root)
            .await
            .unwrap();
        let app = build_router(registry.clone(), credentials);
        async fn request(
            app: &Router,
            method: &str,
            uri: &str,
            body: serde_json::Value,
            auth: bool,
        ) -> (StatusCode, serde_json::Value) {
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json");
            if auth {
                request = request.header("authorization", "Bearer fixture-auth");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            )
        }
        let configure = "/v1/providers/fixture/credential";
        let create = json!({"provider":"fixture","template":"image:alpine:3.24.1","idempotency_key":Uuid::new_v4()});
        let first = json!({"TOKEN":"account-a-v1"});
        assert_eq!(
            request(&app, "POST", "/v1/sandboxes", create.clone(), true)
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            request(&app, "PUT", configure, first.clone(), false)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "PUT",
                "/v1/providers/fixture/configure",
                json!({"credential":first,"options":{}}),
                true
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/providers/fixture/verify",
                json!({}),
                false
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/v1/providers/fixture/verify",
                json!({}),
                true
            )
            .await
            .0,
            StatusCode::OK
        );
        let configured = database.binding(None, None, "fixture").await.unwrap();
        assert_eq!(configured.configuration["MAGICMTN_FIXTURE_APP_ID"], "app-a");
        assert_eq!(
            request(
                &app,
                "PUT",
                "/v1/providers/fixture/configure",
                json!({"credential":{"TOKEN":"account-a-v1"},"options":{}}),
                true
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            database
                .binding(None, None, "fixture")
                .await
                .unwrap()
                .generation,
            configured.generation
        );
        assert_eq!(
            request(
                &app,
                "PUT",
                "/v1/providers/fixture/configure",
                json!({"credential":{"TOKEN":"account-a-v1"},"options":{"unknown":"value"}}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (status, sandbox) = request(&app, "POST", "/v1/sandboxes", create.clone(), true).await;
        assert_eq!(status, StatusCode::OK);
        let exec = format!("/v1/sandboxes/{}/exec", sandbox["id"].as_str().unwrap());
        let command = json!({"command":"echo","args":[],"timeout_ms":1000,"max_output_bytes":1024});
        assert_eq!(
            request(&app, "POST", &exec, command.clone(), true).await.1["stdout"],
            "first"
        );
        assert_eq!(
            request(
                &app,
                "PUT",
                configure,
                json!({"TOKEN":"account-b-v1"}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&app, "POST", &exec, command.clone(), true).await.1["stdout"],
            "first"
        );
        struct BlockedSource {
            started: tokio::sync::Notify,
            released: tokio::sync::Notify,
        }
        #[axum::async_trait]
        impl magicmtn_engine::provider_credentials::CredentialSource for BlockedSource {
            async fn load(
                &self,
            ) -> Result<magicmtn_engine::provider_credentials::CredentialBundle> {
                self.started.notify_one();
                self.released.notified().await;
                Ok(magicmtn_engine::provider_credentials::CredentialBundle {
                    version: "delayed-read".into(),
                    values: parse_bundle(br#"{"TOKEN":"account-a-v1"}"#)?,
                })
            }
        }
        let source = Arc::new(BlockedSource {
            started: Default::default(),
            released: Default::default(),
        });
        let delayed = tokio::spawn({
            let binding = database.binding(None, None, "fixture").await.unwrap();
            let source = source.clone();
            let registry = registry.clone();
            async move {
                magicmtn_engine::provider_credentials::refresh(
                    &registry,
                    &binding,
                    source.as_ref(),
                    "a",
                    std::time::Duration::from_secs(60),
                )
                .await
            }
        });
        let deadline = std::time::Duration::from_secs(5);
        tokio::time::timeout(deadline, source.started.notified())
            .await
            .unwrap();
        let mut another = create;
        another["idempotency_key"] = json!(Uuid::new_v4());
        tokio::time::timeout(deadline, async {
            assert_eq!(
                request(&app, "POST", "/v1/sandboxes", another, true)
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(
                request(&app, "POST", &exec, command.clone(), true).await.1["stdout"],
                "first"
            );
        })
        .await
        .unwrap();
        assert_eq!(
            request(
                &app,
                "PUT",
                configure,
                json!({"TOKEN":"account-a-v2"}),
                true
            )
            .await
            .0,
            StatusCode::OK
        );
        source.released.notify_one();
        assert!(delayed.await.unwrap().is_err());
        assert_eq!(
            request(&app, "POST", &exec, command.clone(), true).await.1["stdout"],
            "second"
        );
        let ciphertext: Vec<u8> = sqlx::query_scalar("SELECT ciphertext FROM provider_credentials")
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert!(!ciphertext
            .windows(b"account-a-v2".len())
            .any(|part| part == b"account-a-v2"));
        let replacement = Registry::new(providers).unwrap();
        database
            .bootstrap_with_configuration(
                &replacement,
                &[(
                    "fixture".into(),
                    [("MAGICMTN_FIXTURE_APP_ID".into(), "replacement-app".into())].into(),
                )]
                .into(),
            )
            .await
            .unwrap();
        assert_eq!(
            database
                .binding(None, None, "fixture")
                .await
                .unwrap()
                .configuration["MAGICMTN_FIXTURE_APP_ID"],
            "app-a"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("setup-calls")).unwrap(),
            "setup\nsetup\n"
        );
        let credentials = LocalCredentials::open(database.clone(), replacement.clone(), root)
            .await
            .unwrap();
        let restarted = build_router(replacement, credentials);
        assert_eq!(
            request(&restarted, "POST", &exec, command, true).await.1["stdout"],
            "second"
        );
    }
}
