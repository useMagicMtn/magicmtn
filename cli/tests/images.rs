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
    // A saved local selection must not redirect the default Cloud image listing.
    fs::write(
        credentials.join("selection.json"),
        json!({"mode": "local", "directory": home.path().join("stopped-local-server")}).to_string(),
    )
    .unwrap();
    for explicit in [false, true] {
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

#[cfg(target_os = "macos")]
#[test]
fn local_inventory_needs_neither_server_nor_cloud_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let runtime = bin.join("container");
    fs::write(
        &runtime,
        "#!/bin/sh\n[ \"$*\" = 'image list' ] || exit 9\necho local-image\n",
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    let output = command(home.path())
        .env("PATH", &bin)
        .env("MAGICMTN_SERVER_URL", "invalid-server-url")
        .args(["--local", "images", "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "local-image\n");
}

#[cfg(target_os = "linux")]
#[test]
fn local_inventory_lists_firecracker_mapping_without_a_server() {
    let home = tempfile::tempdir().unwrap();
    let output = command(home.path())
        .env(
            "MAGICMTN_FIRECRACKER_IMAGES",
            r#"{"example/image@sha256:123":"prepared-vm"}"#,
        )
        .args(["--local", "images", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("prepared-vm"));
}
