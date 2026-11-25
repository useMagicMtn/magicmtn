use axum::{http::StatusCode, routing::get, Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

fn command(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magic"));
    command
        .env("HOME", home)
        .env_remove("MAGICMTN_SERVER_URL")
        .env_remove("MAGICMTN_CA_CERT")
        .env_remove("MAGICMTN_CONFIG_DIRECTORY")
        .env_remove("MAGICMTN_LOCAL_DIRECTORY");
    command
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloud_inventory_uses_scoped_auth_and_excludes_disabled_versions() {
    let app = Router::new().route(
        "/v1/images",
        get(|headers: axum::http::HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer image-test-token");
            Json(json!({"images": [
                {"id": "11111111-1111-4111-8111-111111111111", "display_name": "MagicMtn Canonical",
                 "description": "Current Cloud release", "is_default": true, "status": "enabled"},
                {"id": "22222222-2222-4222-8222-222222222222", "display_name": "Retired version",
                 "description": "Old release", "is_default": false, "status": "disabled"}
            ]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home = tempfile::tempdir().unwrap();
    let credentials = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&credentials).unwrap();
    fs::write(
        credentials.join(format!("{:x}.json", Sha256::digest(base.as_bytes()))),
        json!({"server": base, "token": "image-test-token"}).to_string(),
    )
    .unwrap();
    for explicit in [false, true] {
        // Explicit server selection overrides a saved local deployment.
        let selected = if explicit {
            json!({"mode":"local", "directory":home.path().join("stopped-local-server")})
        } else {
            json!({"mode":"cloud","server":base})
        };
        fs::write(credentials.join("selection.json"), selected.to_string()).unwrap();
        let mut child = command(home.path());
        child.env("MAGICMTN_SERVER_URL", &base);
        if explicit {
            child.args(["--server-url", &base]);
        }
        let output = child.args(["images", "list"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("MagicMtn Canonical"));
        assert!(text.contains("11111111-1111-4111-8111-111111111111"));
        assert!(text.contains("yes"));
        assert!(!text.contains("Retired version"));
        assert!(!text.contains("22222222-2222-4222-8222-222222222222"));
    }
    server.abort();
}

#[test]
fn cloud_inventory_requires_sign_in() {
    let home = tempfile::tempdir().unwrap();
    let output = command(home.path())
        .args(["images", "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("magic login"));
    assert!(output.stdout.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloud_inventory_does_not_fall_back_on_an_expired_session() {
    let app = Router::new().route("/v1/images", get(|| async { StatusCode::UNAUTHORIZED }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home = tempfile::tempdir().unwrap();
    let credentials = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&credentials).unwrap();
    fs::write(
        credentials.join(format!("{:x}.json", Sha256::digest(base.as_bytes()))),
        json!({"server": base, "token": "expired"}).to_string(),
    )
    .unwrap();
    let output = command(home.path())
        .args(["--server-url", &base, "images", "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("session expired"));
    assert!(output.stdout.is_empty());
    server.abort();
}

#[test]
fn local_catalog_includes_canonical_without_a_server_runtime_or_cloud_credentials() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("selection.json"),
        json!({"mode":"local","directory":home.path().join(".config/magicmtn/local")}).to_string(),
    )
    .unwrap();
    let output = command(home.path())
        .env("PATH", home.path())
        .env("MAGICMTN_SERVER_URL", "invalid-server-url")
        .args(["images", "list", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["images"].as_array().unwrap().len(), 1);
    assert_eq!(value["images"][0]["name"], "canonical");
    assert_eq!(value["images"][0]["is_default"], true);
    assert_eq!(
        value["images"][0]["manifests"].as_object().unwrap().len(),
        2
    );
    assert!(value["images"][0]["preparations"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn add_and_prepare_require_a_standalone_deployment() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["images", "add", "dev", "--from", "ghcr.io/example/dev:1"],
        vec!["images", "prepare", "canonical", "--provider", "modal"],
    ] {
        let output = command(home.path()).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("magic server start"));
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_uses_only_the_local_server_and_private_credential() {
    use axum::routing::post;
    use std::os::unix::fs::PermissionsExt;
    let app = Router::new().route(
        "/v1/images/my-dev/prepare",
        post(
            |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
                assert_eq!(headers["authorization"], "Bearer local-only-token");
                assert_eq!(body, json!({"provider":"modal"}));
                Json(json!({"prepared":{"platform":"linux/amd64","artifact":"im-test"}}))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let home = tempfile::tempdir().unwrap();
    let local = home.path().join("server");
    fs::create_dir(&local).unwrap();
    let connection = local.join("connection.json");
    fs::write(
        &connection,
        json!({"url":url,"token":"local-only-token"}).to_string(),
    )
    .unwrap();
    fs::set_permissions(connection, fs::Permissions::from_mode(0o600)).unwrap();
    let state = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&state).unwrap();
    fs::write(
        state.join("selection.json"),
        json!({"mode":"local","directory":local}).to_string(),
    )
    .unwrap();
    let output = command(home.path())
        .env("MAGICMTN_LOCAL_DIRECTORY", local)
        .env("MAGICMTN_SERVER_URL", "invalid-cloud-url")
        .args(["images", "prepare", "my-dev", "--provider", "modal"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("my-dev is ready on modal (linux/amd64)")
    );
    server.abort();
}
