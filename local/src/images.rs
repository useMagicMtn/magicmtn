//! Local image registration and credential-scoped preparation, independent of Cloud.
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use magicmtn_engine::{
    sandbox_images::{self, Manifests},
    sandbox_storage::{sqlite::Database, Store},
};
use magicmtn_provider_framework::host::Registry;
use magicmtn_sdk::{
    image_registry::Image,
    plugin::{Operation, Value as ProviderValue},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path as FilePath, sync::Arc, time::Duration};

async fn open(directory: &FilePath) -> Result<Database> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = directory.join("magicmtn.sqlite3");
    let db = Database::open(&path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(db)
}

pub async fn catalog(directory: &FilePath) -> Result<Value> {
    let db = open(directory).await?;
    sandbox_images::list(&db).await
}

pub async fn add(directory: &FilePath, name: &str, source: &str) -> Result<Value> {
    ensure!(
        sandbox_images::valid_name(name) && name != "canonical",
        "invalid or reserved image name"
    );
    let manifests = resolve(source).await?;
    let db = open(directory).await?;
    sandbox_images::seed(&db).await?;
    sandbox_images::add(&db, name, source, &manifests).await?;
    Ok(json!({"name":name,"manifests":manifests}))
}

#[derive(Clone)]
struct Service {
    db: Database,
    registry: Registry,
    preparations: Arc<tokio::sync::Mutex<()>>,
}

pub(crate) fn router(db: Database, registry: Registry) -> Router {
    Router::new()
        .route("/v1/images", get(list))
        .route("/v1/images/:name/prepare", post(prepare))
        .with_state(Service {
            db,
            registry,
            preparations: Default::default(),
        })
}

type Reply = std::result::Result<Json<Value>, (StatusCode, String)>;
fn failure(error: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, error.to_string())
}

async fn list(State(service): State<Service>) -> Reply {
    sandbox_images::list(&service.db)
        .await
        .map(Json)
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    provider: String,
}

async fn prepare(
    State(service): State<Service>,
    Path(name): Path<String>,
    Json(body): Json<Prepare>,
) -> Reply {
    async {
        let _lock = service.preparations.lock().await;
        let binding = service.db.binding(None, None, &body.provider).await?;
        magicmtn_engine::provider_connections::ensure_sandbox(&binding)?;
        // Admit credentials even on cache hits so an unconfigured account is never ready.
        if binding.manifest.credential.is_some() {
            let reference = magicmtn_engine::provider_credentials::reference(&binding)?;
            service.registry.admit_credential(&reference, &binding.pin)?;
        }
        if let Some(result) = sandbox_images::prepared(&service.db, &name, &binding).await? {
            return Ok(Json(json!({"name":name,"provider":body.provider,"cached":true,"prepared":result})));
        }
        let images = sandbox_images::manifests(&service.db, &name).await?;
        let operation = Operation::SandboxPrepareImage { images };
        ensure!(binding.manifest.capabilities.supports(&operation), "installed provider does not support image preparation; install a provider release with prepare_image support");
        let scope = format!("image-prepare:{}:{}", binding.connection_id, uuid::Uuid::new_v4());
        let result = magicmtn_engine::provider_connections::dispatch_store(&Store::Sqlite(service.db.clone()), &service.registry, &binding, &scope, operation, Duration::from_secs(1800), false).await;
        service.registry.release(&scope).await;
        let ProviderValue::PreparedImage(result) = result? else { anyhow::bail!("provider returned an invalid preparation result"); };
        sandbox_images::save(&service.db, &name, &binding, &result).await?;
        Ok(Json(json!({"name":name,"provider":body.provider,"cached":false,"prepared":result})))
    }.await.map_err(failure)
}

const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";

struct RegistryClient {
    client: reqwest::Client,
    base: String,
    repository: String,
    token: Option<String>,
}
impl RegistryClient {
    async fn fetch(&mut self, resource: &str, digest: Option<&str>) -> Result<Vec<u8>> {
        if let Some(digest) = digest {
            ensure!(
                digest
                    .strip_prefix("sha256:")
                    .is_some_and(|hash| hash.len() == 64
                        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())),
                "invalid registry descriptor digest"
            );
        }
        let url = format!("{}/v2/{}/{}", self.base, self.repository, resource);
        let mut request = self.client.get(&url).header("Accept", ACCEPT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let mut response = request.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let challenge = response
                .headers()
                .get("www-authenticate")
                .context("registry requires authentication")?
                .to_str()?;
            let fields: std::collections::BTreeMap<_,_> = challenge.strip_prefix("Bearer ").context("registry requires credentials; use a publicly readable image or configure registry access")?.split(',')
                .filter_map(|part| part.trim().split_once('=').map(|(key,value)|(key.to_owned(),value.trim_matches('"').to_owned()))).collect();
            let mut realm = reqwest::Url::parse(
                fields
                    .get("realm")
                    .context("registry authentication realm missing")?,
            )?;
            ensure!(
                realm.username().is_empty()
                    && realm.password().is_none()
                    && (realm.scheme() == "https"
                        || (realm.scheme() == "http"
                            && realm.host_str() == Some("127.0.0.1")
                            && self.base.starts_with("http://127.0.0.1:"))),
                "invalid registry authentication realm"
            );
            {
                let mut query = realm.query_pairs_mut();
                for key in ["service", "scope"] {
                    if let Some(value) = fields.get(key) {
                        query.append_pair(key, value);
                    }
                }
            }
            let auth: Value = self
                .client
                .get(realm)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let token = auth["token"]
                .as_str()
                .or_else(|| auth["access_token"].as_str())
                .context("registry did not grant image access")?
                .to_owned();
            response = self
                .client
                .get(&url)
                .header("Accept", ACCEPT)
                .bearer_auth(&token)
                .send()
                .await?;
            self.token = Some(token);
        }
        // Registries may redirect blobs to a CDN. Never forward registry tokens.
        for _ in 0..5 {
            if !response.status().is_redirection() {
                break;
            }
            let target = response.url().join(
                response
                    .headers()
                    .get("location")
                    .context("registry redirect missing location")?
                    .to_str()?,
            )?;
            ensure!(
                target.username().is_empty()
                    && target.password().is_none()
                    && (target.scheme() == "https"
                        || (target.scheme() == "http"
                            && self.base.starts_with("http://127.0.0.1:")
                            && target.host_str() == Some("127.0.0.1"))),
                "invalid registry redirect"
            );
            response = self
                .client
                .get(target)
                .header("Accept", ACCEPT)
                .send()
                .await?;
        }
        ensure!(
            response.status().is_success(),
            "could not retrieve image; check registry access"
        );
        response = response
            .error_for_status()
            .context("could not retrieve image; check the reference and registry access")?;
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= 4 * 1024 * 1024),
            "registry manifest is too large"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 4 * 1024 * 1024,
                "registry manifest is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        if let Some(digest) = digest {
            ensure!(
                format!("sha256:{:x}", Sha256::digest(&bytes)) == digest,
                "registry content does not match its digest"
            );
        }
        Ok(bytes)
    }
}

pub async fn resolve(source: &str) -> Result<Manifests> {
    ensure!(
        !source.is_empty()
            && source.len() <= 512
            && !source.starts_with('-')
            && source
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_.:@-".contains(&c)),
        "use an OCI repository with an explicit tag or SHA-256 digest"
    );
    let (repository, version) = if let Some(pair) = source.split_once('@') {
        pair
    } else {
        let (repository, tag) = source
            .rsplit_once(':')
            .context("provide an explicit image tag or SHA-256 digest")?;
        ensure!(!tag.contains('/'), "provide an explicit image tag");
        (repository, tag)
    };
    ensure!(
        !repository.is_empty() && !version.is_empty(),
        "invalid image reference"
    );
    let pinned = source.contains('@');
    if pinned {
        ensure!(
            version
                .strip_prefix("sha256:")
                .is_some_and(|h| h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit())),
            "invalid image digest"
        );
    }
    let (host, path) = match repository.split_once('/') {
        Some((host, path)) if host.contains('.') || host.contains(':') || host == "localhost" => {
            (host.to_owned(), path.to_owned())
        }
        _ => (
            "docker.io".into(),
            if repository.contains('/') {
                repository.to_owned()
            } else {
                format!("library/{repository}")
            },
        ),
    };
    ensure!(
        !path.is_empty()
            && !path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."),
        "invalid repository path"
    );
    let address = if host == "docker.io" {
        "registry-1.docker.io"
    } else {
        &host
    };
    let scheme = if address.starts_with("localhost:") || address.starts_with("127.0.0.1:") {
        "http"
    } else {
        "https"
    };
    let mut registry = RegistryClient {
        client: reqwest::Client::builder()
            .no_proxy()
            .user_agent("MagicMtn image catalog")
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?,
        base: format!("{scheme}://{address}"),
        repository: path.clone(),
        token: None,
    };
    let bytes = registry
        .fetch(&format!("manifests/{version}"), pinned.then_some(version))
        .await?;
    let root: Value = serde_json::from_slice(&bytes)?;
    let mut descriptors = Vec::new();
    if let Some(entries) = root["manifests"].as_array() {
        for entry in entries {
            if entry["platform"]["os"] == "linux"
                && matches!(
                    entry["platform"]["architecture"].as_str(),
                    Some("amd64" | "arm64")
                )
            {
                let digest = entry["digest"]
                    .as_str()
                    .context("image index descriptor missing digest")?;
                let bytes = registry
                    .fetch(&format!("manifests/{digest}"), Some(digest))
                    .await?;
                descriptors.push((digest.to_owned(), serde_json::from_slice::<Value>(&bytes)?));
            }
        }
    } else {
        descriptors.push((format!("sha256:{:x}", Sha256::digest(&bytes)), root));
    }
    let mut manifests = Manifests::new();
    for (digest, manifest) in descriptors {
        let config = manifest["config"]["digest"]
            .as_str()
            .context("image manifest missing configuration")?;
        let bytes = registry
            .fetch(&format!("blobs/{config}"), Some(config))
            .await?;
        let configuration: Value = serde_json::from_slice(&bytes)?;
        let platform = format!(
            "{}/{}",
            configuration["os"].as_str().unwrap_or(""),
            configuration["architecture"].as_str().unwrap_or("")
        );
        if !matches!(platform.as_str(), "linux/arm64" | "linux/amd64") {
            continue;
        }
        ensure!(
            manifests
                .insert(
                    platform,
                    Image {
                        repository: format!("{host}/{path}"),
                        digest
                    }
                )
                .is_none(),
            "image index contains duplicate platform images"
        );
    }
    ensure!(
        !manifests.is_empty(),
        "image has no supported Linux platform"
    );
    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use std::collections::BTreeMap;
    use tower::ServiceExt;

    fn digest(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    async fn registry_fixture() -> (String, String, Manifests, tokio::task::JoinHandle<()>) {
        let mut resources = BTreeMap::new();
        let mut entries = Vec::new();
        let mut expected = Manifests::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        for arch in ["arm64", "amd64"] {
            let config = serde_json::to_vec(&json!({"os":"linux","architecture":arch})).unwrap();
            let config_digest = digest(&config);
            resources.insert(format!("/v2/test/dev/blobs/{config_digest}"), config);
            let manifest =
                serde_json::to_vec(&json!({"config":{"digest":config_digest},"layers":[]}))
                    .unwrap();
            let manifest_digest = digest(&manifest);
            resources.insert(
                format!("/v2/test/dev/manifests/{manifest_digest}"),
                manifest,
            );
            entries.push(
                json!({"digest":manifest_digest,"platform":{"os":"linux","architecture":arch}}),
            );
            expected.insert(
                format!("linux/{arch}"),
                Image {
                    repository: format!("{address}/test/dev"),
                    digest: manifest_digest,
                },
            );
        }
        let index = serde_json::to_vec(&json!({"manifests":entries})).unwrap();
        let root_digest = digest(&index);
        resources.insert("/v2/test/dev/manifests/1".to_owned(), index.clone());
        resources.insert(format!("/v2/test/dev/manifests/{root_digest}"), index);
        // A valid-looking digest with different contents must fail verification.
        resources.insert(
            format!("/v2/test/dev/manifests/sha256:{}", "0".repeat(64)),
            b"{}".to_vec(),
        );
        let registry_address = address.clone();
        let app = Router::new().fallback(get(move |uri: axum::extract::OriginalUri, headers:axum::http::HeaderMap| {
            let resources = resources.clone();
            let address=registry_address.clone();
            async move {
                let path=uri.0.path();
                if path=="/token" { return axum::response::Response::builder().body(Body::from(r#"{"token":"fixture-token"}"#)).unwrap(); }
                if let Some(digest)=path.strip_prefix("/cdn/") {
                    assert!(!headers.contains_key("authorization"),"registry token leaked to blob storage");
                    let bytes=resources.get(&format!("/v2/test/dev/blobs/{digest}")).unwrap().clone();
                    return axum::response::Response::builder().body(Body::from(bytes)).unwrap();
                }
                if headers.get("authorization").and_then(|header|header.to_str().ok())!=Some("Bearer fixture-token") {
                    return axum::response::Response::builder().status(StatusCode::UNAUTHORIZED)
                        .header("www-authenticate",format!("Bearer realm=\"http://{address}/token\",service=\"test\",scope=\"repository:test/dev:pull\""))
                        .body(Body::empty()).unwrap();
                }
                if let Some(digest)=path.strip_prefix("/v2/test/dev/blobs/") {
                    return axum::response::Response::builder().status(StatusCode::TEMPORARY_REDIRECT)
                        .header("location",format!("http://{address}/cdn/{digest}")).body(Body::empty()).unwrap();
                }
                let bytes=resources.get(path).cloned();
                axum::response::Response::builder().status(if bytes.is_some() {StatusCode::OK} else {StatusCode::NOT_FOUND}).body(Body::from(bytes.unwrap_or_default())).unwrap()
            }
        }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("{address}/test/dev"), root_digest, expected, task)
    }

    #[tokio::test]
    async fn add_resolves_all_platforms_pins_digests_and_survives_reopen() {
        let (source, root, expected, server) = registry_fixture().await;
        assert_eq!(
            resolve(&format!("{source}@{root}")).await.unwrap(),
            expected
        );
        let directory = tempfile::tempdir().unwrap();
        add(directory.path(), "my-dev", &format!("{source}:1"))
            .await
            .unwrap();
        add(directory.path(), "my-dev", &format!("{source}:1"))
            .await
            .unwrap();
        let catalog = catalog(directory.path()).await.unwrap();
        assert_eq!(catalog["images"].as_array().unwrap().len(), 2);
        assert_eq!(catalog["images"][1]["manifests"], json!(expected));
        server.abort();
        // Listing the saved catalog never revisits its registry.
        assert_eq!(self::catalog(directory.path()).await.unwrap(), catalog);
    }

    #[tokio::test]
    async fn incorrect_digest_is_rejected_before_registration() {
        let (source, _, _, server) = registry_fixture().await;
        let directory = tempfile::tempdir().unwrap();
        assert!(add(
            directory.path(),
            "bad",
            &format!("{source}@sha256:{}", "0".repeat(64))
        )
        .await
        .is_err());
        assert_eq!(
            catalog(directory.path()).await.unwrap()["images"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        server.abort();
    }

    #[tokio::test]
    async fn implicit_tags_and_invalid_names_are_rejected_without_registry_access() {
        for source in [
            "alpine",
            "ghcr.io/example/dev",
            "https://example/image:1",
            "example/image@not-a-digest",
        ] {
            assert!(resolve(source).await.is_err(), "{source}");
        }
        let directory = tempfile::tempdir().unwrap();
        assert!(add(directory.path(), "canonical", "no-network:1")
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepare_api_caches_provider_result_and_launches_with_frozen_image() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        use magicmtn_sdk::plugin::{Capabilities, ProviderManifest, SandboxOperation};
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let providers = directory.path().join("providers");
        let release = providers.join("fixture/1.0.0");
        std::fs::create_dir_all(&release).unwrap();
        let manifest = ProviderManifest {
            manifest_version: 1,
            provider_version: "1.0.0".into(),
            credential: None,
            capabilities: Capabilities::sandbox(vec![
                SandboxOperation::PrepareImage,
                SandboxOperation::Create,
                SandboxOperation::ExecBounded,
                SandboxOperation::Terminate,
            ]),
        };
        let image = magicmtn_sdk::image_registry::canonical("linux/amd64").unwrap();
        let prepared = json!({"kind":"prepared_image","value":{"image":image,"platform":"linux/amd64","artifact":"fixture-image","configuration":{}}});
        let script = format!(
            r#"#!/bin/sh
reply() {{ printf '{{"id":%s,"protocol_version":1,"provider":"fixture","outcome":{{"ok":%s}}}}\n' "$1" "$2"; }}
IFS= read -r frame || exit 1
reply 1 '{}'
id=1
while IFS= read -r frame; do
  id=$((id+1))
  case "$frame" in
    *sandbox_prepare_image*) reply "$id" '{}' ;;
    *sandbox_create*) reply "$id" '{{"kind":"sandbox","value":{{"provider_id":"fixture-sandbox","status":"running","create_time_ms":null,"metadata":null}}}}' ;;
    *shutdown*) reply "$id" '{{"kind":"done"}}'; exit ;;
    *) exit 1 ;;
  esac
done
"#,
            json!({"kind":"description","value":manifest}),
            prepared
        );
        let hash = format!("{:x}", Sha256::digest(script.as_bytes()));
        let package = json!({"name":"fixture","version":"1.0.0","protocol_version":1,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"sha256":hash});
        std::fs::write(release.join("provider"), script).unwrap();
        std::fs::set_permissions(
            release.join("provider"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(release.join("package.json"), package.to_string()).unwrap();
        std::fs::write(release.join("manifest.json"), json!(manifest).to_string()).unwrap();
        std::fs::write(providers.join("fixture/active"), "1.0.0").unwrap();
        let registry = Registry::new(providers).unwrap();
        let db = Database::open(&directory.path().join("local.sqlite"))
            .await
            .unwrap();
        db.bootstrap(&registry).await.unwrap();
        sandbox_images::seed(&db).await.unwrap();
        let app = super::router(db.clone(), registry.clone());
        async fn request(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice(&bytes)
                    .unwrap_or(json!({"error":String::from_utf8_lossy(&bytes)})),
            )
        }
        let (status, result) = request(
            &app,
            "/v1/images/canonical/prepare",
            json!({"provider":"fixture"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["cached"], false);
        let (status, result) = request(
            &app,
            "/v1/images/canonical/prepare",
            json!({"provider":"fixture"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["cached"], true);
        let sandbox_app = crate::router(
            Store::Sqlite(db.clone()),
            registry.clone(),
            "test-token".into(),
            60,
        );
        let response = sandbox_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sandboxes")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"provider":"fixture","template":"image:canonical"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let saved: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["template"], format!("image:{}", image.reference()));
        assert_eq!(saved["metadata"]["_magicmtn_image_selection"], "canonical");
        assert_eq!(
            sandbox_images::list(&db).await.unwrap()["images"][0]["preparations"][0]["ready"],
            true
        );
        registry.shutdown().await;
    }
}
