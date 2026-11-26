use std::fs::{self, File};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn magic(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magic"));
    command
        .env("HOME", home)
        .env_remove("MAGICMTN_CONFIG_DIRECTORY")
        .env_remove("MAGICMTN_LOCAL_DIRECTORY")
        .env_remove("MAGICMTN_SERVER_URL")
        .env_remove("MAGICMTN_DEPLOYMENT_MODE")
        .env_remove("MAGICMTN_LOCAL_DATABASE_URL");
    command
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn running_server_blocks_login_and_stop_releases_locks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/local");
    let credentials = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&credentials).unwrap();
    use sha2::{Digest, Sha256};
    let developer = "http://127.0.0.1:0/";
    fs::write(
        credentials.join(format!("{:x}.json", Sha256::digest(developer.as_bytes()))),
        serde_json::json!({"server":developer,"token":"developer-session"}).to_string(),
    )
    .unwrap();
    let log = home.path().join("server.log");
    let mut server = Server(
        magic(home.path())
            .args([
                "server",
                "start",
                "--bind",
                "127.0.0.1:0",
                "--ui-bind",
                "127.0.0.1:0",
            ])
            .stdout(Stdio::null())
            .stderr(File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !fs::read_to_string(&log).unwrap().contains("Password file:") {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited during startup"
        );
        assert!(Instant::now() < deadline, "server startup timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
    let startup = fs::read_to_string(&log).unwrap();
    let ui = startup
        .lines()
        .find_map(|line| line.strip_prefix("MagicMtn web UI listening on "))
        .unwrap();
    let entry = startup
        .lines()
        .find_map(|line| line.strip_prefix("Dashboard username: magic  Password file: "))
        .unwrap();
    let password = fs::read_to_string(entry).unwrap();
    let client = reqwest::Client::new();
    let catalog = format!("{ui}/compute/images");
    assert_eq!(client.get(&catalog).send().await.unwrap().status(), 401);
    let html = client
        .get(&catalog)
        .basic_auth("magic", Some(password.trim()))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("canonical"));
    for (path, marker) in [
        ("/compute/computers", "<h1>Computers</h1>"),
        ("/providers", "Installed releases"),
        ("/templates", "template-toml"),
        ("/mcp", "mcp-add-sentry"),
    ] {
        assert!(html.contains(&format!("href=\"{path}\"")));
        let page = client
            .get(format!("{ui}{path}"))
            .basic_auth("magic", Some(password.trim()))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(page.contains(marker), "{path}");
    }
    let save_url = format!("{ui}/api/templates/default");
    let template = serde_json::json!({"toml": "name='default'\ndescription='Test workspace'\n[repos]\ngithub=['example/project']"});
    assert_eq!(
        client
            .put(&save_url)
            .json(&template)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    client
        .put(&save_url)
        .basic_auth("magic", Some(password.trim()))
        .json(&template)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let invalid = serde_json::json!({"toml": "name='unavailable'"});
    assert_eq!(
        client
            .put(&save_url)
            .basic_auth("magic", Some(password.trim()))
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    let saved = client
        .get(format!("{ui}/templates"))
        .basic_auth("magic", Some(password.trim()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(saved.contains("Test workspace") && saved.contains("example/project"));
    for path in ["/billing", "/account", "/usage", "/registry"] {
        assert!(!html.contains(&format!("href=\"{path}\"")));
        assert_eq!(
            client
                .get(format!("{ui}{path}"))
                .basic_auth("magic", Some(password.trim()))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert!(magicmtn_local::is_running(&directory).unwrap());
    let account = magic(home.path()).args(["pi", "account"]).output().unwrap();
    assert!(
        account.status.success(),
        "{}",
        String::from_utf8_lossy(&account.stderr)
    );
    let account: serde_json::Value = serde_json::from_slice(&account.stdout).unwrap();
    assert_eq!(account["mode"], "local");
    assert_eq!(account["signed_in"], false);
    let list = magic(home.path())
        .args(["computer", "list"])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let development = magic(home.path())
        .args(["--server-url", "http://127.0.0.1:0", "login"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&development.stderr);
    assert!(error.contains("authentication service"), "{error}");
    assert!(!error.contains("magic server stop"), "{error}");

    let login = magic(home.path()).arg("login").output().unwrap();
    assert!(!login.status.success());
    assert!(String::from_utf8_lossy(&login.stderr).contains("magic server stop"));
    let other_directory = tempfile::tempdir().unwrap();
    let overridden = magic(home.path())
        .env("MAGICMTN_LOCAL_DIRECTORY", other_directory.path())
        .arg("login")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&overridden.stderr).contains("magic server stop"));
    let stop = magic(home.path())
        .env("MAGICMTN_LOCAL_DIRECTORY", other_directory.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    server.0.wait().unwrap();
    assert!(client.get(&catalog).send().await.is_err());
    assert!(!magicmtn_local::is_running(&directory).unwrap());
    assert!(directory.join("connection.json").exists());
    let account = magic(home.path()).args(["pi", "account"]).output().unwrap();
    assert!(account.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&account.stdout).unwrap()["mode"],
        "local"
    );

    // Stale connection and lock files do not prevent the next login attempt.
    let login = magic(home.path())
        .args(["--server-url", "http://127.0.0.1:0", "login"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&login.stderr);
    assert!(error.contains("authentication service"), "{error}");
    assert!(!error.contains("magic server stop"));
}

#[test]
fn transition_lock_blocks_both_login_and_startup() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&directory).unwrap();
    let lock = File::create(directory.join("mode.lock")).unwrap();
    lock.lock().unwrap();
    for args in [vec!["login"], vec!["server", "start"]] {
        let output = magic(home.path()).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("in progress"));
    }
}

#[tokio::test]
async fn cloud_authentication_blocks_local_start_before_initialization() {
    use axum::{routing::get, Json, Router};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let app = Router::new().route(
        "/auth/me",
        get(|| async { Json(json!({"display_name":"Person","email":"person@example.test"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&directory).unwrap();
    let digest = Sha256::digest(url.as_bytes());
    fs::write(
        directory.join(format!("{digest:x}.json")),
        json!({"server":url,"token":"valid"}).to_string(),
    )
    .unwrap();
    fs::write(
        directory.join("selection.json"),
        json!({"mode":"cloud", "server":url}).to_string(),
    )
    .unwrap();
    let path = home.path().to_owned();
    let output = tokio::task::spawn_blocking(move || {
        magic(&path).args(["server", "start"]).output().unwrap()
    })
    .await
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("logout"));
    assert!(!home.path().join(".config/magicmtn/local").exists());
    server.abort();
}

#[test]
fn failed_start_does_not_select_local_mode() {
    let home = tempfile::tempdir().unwrap();
    let output = magic(home.path())
        .args(["server", "start", "--bind", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!home
        .path()
        .join(".config/magicmtn/cli/selection.json")
        .exists());
}

#[cfg(unix)]
#[test]
fn standalone_provider_workflow_uses_the_selected_deployment_without_local_flags() {
    use magicmtn_sdk::{
        image_registry,
        plugin::{Capabilities, CredentialSpec, ProviderManifest, SandboxOperation},
        provider_setup::SetupSpec,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};

    let home = tempfile::tempdir().unwrap();
    let calls = home.path().join("provider-calls");
    let mut capabilities = Capabilities::sandbox(vec![
        SandboxOperation::PrepareImage,
        SandboxOperation::Create,
        SandboxOperation::Status,
        SandboxOperation::ExecBounded,
        SandboxOperation::Terminate,
    ]);
    capabilities.setup = Some(SetupSpec {
        options: Default::default(),
    });
    let manifest = ProviderManifest {
        manifest_version: 1,
        provider_version: "1.0.0".into(),
        credential: Some(CredentialSpec {
            fields: vec!["TOKEN".into()],
        }),
        capabilities,
    };
    let image = image_registry::canonical("linux/amd64").unwrap();
    let script = r#"#!/bin/sh
reply() { printf '{"id":%s,"protocol_version":2,"provider":"fixture","outcome":{"ok":%s}}\n' "$1" "$2"; }
IFS= read -r frame || exit 1
case "$frame" in
 *'"verify_account":true'*) reply 1 '{"kind":"verified_account","value":"account-fixture"}'; exit ;;
esac
reply 1 '__MANIFEST__'
id=1
while IFS= read -r frame; do
 id=$((id+1))
 case "$frame" in
  *provider_configure*) reply "$id" '{"kind":"configured_connection","value":{"account":"account-fixture","configuration":{"MAGICMTN_FIXTURE_APP_ID":"app-fixture"}}}' ;;
  *sandbox_prepare_image*) printf 'prepare\n' >> '__CALLS__'; reply "$id" '__PREPARED__' ;;
  *sandbox_create*) reply "$id" '{"kind":"sandbox","value":{"provider_id":"fixture-sandbox","status":"running","create_time_ms":null,"metadata":null}}' ;;
  *sandbox_status*) reply "$id" '{"kind":"sandbox_status","value":"running"}' ;;
  *sandbox_exec_bounded*) reply "$id" '{"kind":"exec","value":{"stdout":"hello","stderr":"","exit_code":0,"elapsed_ms":1,"timed_out":false,"truncated":false}}' ;;
  *sandbox_terminate*) reply "$id" '{"kind":"done"}' ;;
  *shutdown*) reply "$id" '{"kind":"done"}'; exit ;;
  *) exit 1 ;;
 esac
done
"#.replace("__MANIFEST__", &json!({"kind":"description","value":manifest}).to_string())
        .replace("__PREPARED__", &json!({"kind":"prepared_image","value":{"image":image,"platform":"linux/amd64","artifact":"fixture-image","configuration":{}}}).to_string())
        .replace("__CALLS__", &calls.to_string_lossy().replace('\'', "'\\''"));
    let package = json!({"name":"fixture","version":"1.0.0","protocol_version":2,
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,"sha256":format!("{:x}",Sha256::digest(script.as_bytes()))});
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    for (name, data) in [
        ("package.json", package.to_string().into_bytes()),
        ("provider", script.into_bytes()),
        ("LICENSE", b"fixture".to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, name, data.as_slice())
            .unwrap();
    }
    let path = home.path().join("provider.tar.gz");
    fs::write(&path, archive.into_inner().unwrap().finish().unwrap()).unwrap();
    let run = |args: &[&str]| {
        assert!(!args.contains(&"--local"));
        let output = magic(home.path())
            .args(args)
            .env("MAGICMTN_SERVER_URL", "http://127.0.0.1:1")
            .env("PI_CODING_AGENT_DIR", home.path().join("pi"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&["provider", "install", "--file", path.to_str().unwrap()]);
    let mut server = Server(
        magic(home.path())
            .args([
                "server",
                "start",
                "--bind",
                "127.0.0.1:0",
                "--ui-bind",
                "127.0.0.1:0",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let connection = home.path().join(".config/magicmtn/local/connection.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !connection.exists() {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited during startup"
        );
        assert!(Instant::now() < deadline, "server startup timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
    let credentials = home.path().join("credentials.json");
    fs::write(&credentials, r#"{"TOKEN":"fixture"}"#).unwrap();
    run(&[
        "provider",
        "configure",
        "fixture",
        "--secret-file",
        credentials.to_str().unwrap(),
    ]);
    run(&["provider", "verify", "fixture"]);
    assert!(run(&["images", "list"]).contains("canonical"));
    for _ in 0..2 {
        run(&["images", "prepare", "canonical", "--provider", "fixture"]);
    }
    assert_eq!(
        fs::read_to_string(calls).unwrap(),
        "prepare\n",
        "prepared images should be reused"
    );
    let id = run(&[
        "computer",
        "new",
        "--image",
        "canonical",
        "--provider",
        "fixture",
    ]);
    let id = id.trim();
    assert!(run(&["computer", "exec", id, "--", "/bin/echo", "hello"]).contains("hello"));
    run(&["computer", "terminate", id, "--yes"]);
    run(&["agents", "install", "pi"]);
    assert!(home
        .path()
        .join("pi/extensions/magicmtn/index.js")
        .is_file());
    run(&["server", "stop"]);
    assert!(server.0.wait().unwrap().success());
}

#[test]
fn occupied_dashboard_port_prevents_api_startup() {
    let home = tempfile::tempdir().unwrap();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = occupied.local_addr().unwrap().to_string();
    let output = magic(home.path())
        .args([
            "server",
            "start",
            "--bind",
            "127.0.0.1:0",
            "--ui-bind",
            &address,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("binding dashboard"));
    let config = home.path().join(".config/magicmtn");
    assert!(!config.join("local/connection.json").exists());
    assert!(!config.join("cli/selection.json").exists());
}
