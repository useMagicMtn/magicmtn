use crate::release::{self, Trust};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const DOWNLOADS: &str = "https://magicmtn-cli-993499103295.s3.us-east-2.amazonaws.com";

fn platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        _ => bail!("magic update is not available for this platform"),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn get(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    anyhow::ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= limit as u64),
        "release download is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            chunk.len() <= limit.saturating_sub(bytes.len()),
            "release download is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn update(base: &str, executable: &Path, platform: &str, trust: &Trust) -> Result<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let bytes = get(&client, &format!("{base}/latest.signed.json"), 32 * 1024).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let manifest = release::verify(&bytes, trust, now)?;
    anyhow::ensure!(
        manifest.is_stable(),
        "the updater requires a stable release"
    );
    let release = &manifest.release;
    anyhow::ensure!(
        release::stable_version(release)? >= release::stable_version(env!("CARGO_PKG_VERSION"))?,
        "the signed release is older than this CLI; refusing to downgrade"
    );
    let expected = manifest
        .platforms
        .get(platform)
        .context("no release for this platform")?;
    let installed = sha256(&fs::read(executable).context("reading installed magic")?);
    if installed.eq_ignore_ascii_case(expected) {
        return Ok(format!("magic is already the latest version ({release})"));
    }

    let url = format!("{base}/releases/{release}/magic-{platform}");
    let bytes = get(&client, &url, 128 * 1024 * 1024).await?;
    anyhow::ensure!(
        sha256(&bytes).eq_ignore_ascii_case(expected),
        "downloaded magic failed SHA-256 verification"
    );
    let directory = executable
        .parent()
        .context("magic has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)
        .context("cannot stage update beside the installed magic")?;
    staged.write_all(&bytes)?;
    staged.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    }
    let staged = staged.into_temp_path();
    if platform == "darwin-arm64" {
        release::verify_macos_binary(&staged, trust)?;
    }
    let status = Command::new(&staged)
        .arg("--version")
        .output()
        .context("the downloaded magic could not run")?;
    anyhow::ensure!(
        status.status.success(),
        "the downloaded magic could not run"
    );
    staged
        .persist(executable)
        .context("replacing the installed magic")?;
    Ok(format!("Updated magic to release {release}"))
}

// Execute the installed binary: this updater still has the old command metadata
// in memory after replacing itself.
fn refresh_completions(executable: &Path, home: &Path, fish_directory: &Path) -> Result<()> {
    for (shell, path) in [
        (
            "bash",
            home.join(".local/share/magic/completions/magic.bash"),
        ),
        ("zsh", home.join(".local/share/magic/completions/_magic")),
        ("fish", fish_directory.join("magic.fish")),
    ] {
        if !path.exists() {
            continue;
        }
        let output = Command::new(executable)
            .args(["completion", shell, "--print"])
            .output()
            .with_context(|| format!("generating {shell} completions"))?;
        anyhow::ensure!(
            output.status.success() && !output.stdout.is_empty(),
            "could not generate {shell} completions"
        );
        let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        staged.write_all(&output.stdout)?;
        staged.flush()?;
        staged
            .persist(&path)
            .with_context(|| format!("refreshing {shell} completions"))?;
    }
    Ok(())
}

pub(crate) async fn run() -> Result<PathBuf> {
    let executable = std::env::current_exe()
        .context("locating the installed magic")?
        .canonicalize()
        .context("resolving the installed magic")?;
    println!(
        "{}",
        update(DOWNLOADS, &executable, platform()?, &Trust::pinned()?).await?
    );
    let home =
        PathBuf::from(std::env::var_os("HOME").context("HOME environment variable not set")?);
    refresh_completions(
        &executable,
        &home,
        &crate::completions::fish_directory(&home),
    )
    .context("magic is updated, but refreshing installed completions failed")?;
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn completions_use_installed_binary_and_preserve_files_on_failure() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        let home = directory.path();
        let fish = home.join("custom-config/fish/completions");
        fs::create_dir_all(&fish)?;
        let file = fish.join("magic.fish");
        fs::write(&file, "old completions")?;
        let executable = home.join("magic");
        fs::write(&executable, "#!/bin/sh\n[ \"$1\" = completion ] && [ \"$2\" = fish ] && [ \"$3\" = --print ] || exit 1\necho updated-completions\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        refresh_completions(&executable, home, &fish)?;
        assert_eq!(fs::read_to_string(&file)?, "updated-completions\n");
        assert!(!home.join(".local/share/magic/completions/_magic").exists());
        fs::write(&executable, "#!/bin/sh\nexit 1\n")?;
        assert!(refresh_completions(&executable, home, &fish).is_err());
        assert_eq!(fs::read_to_string(&file)?, "updated-completions\n");
        Ok(())
    }

    #[test]
    fn supported_platform_names_match_publisher() {
        if matches!(std::env::consts::OS, "macos" | "linux") {
            assert!(matches!(
                platform().unwrap(),
                "darwin-arm64" | "linux-x86_64" | "linux-arm64"
            ));
        }
    }

    fn manifest_payload(hash: &str) -> serde_json::Value {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        json!({
            "schema": 1, "release": env!("CARGO_PKG_VERSION"), "channel": "stable",
            "source_commit": "a".repeat(40), "issued_at": now - 1, "expires_at": now + 3600,
            "apple_team_id": "ABCDE12345",
            "platforms": release::PLATFORMS.into_iter().map(|platform| (platform, hash)).collect::<std::collections::BTreeMap<_,_>>()
        })
    }

    fn signed_manifest(hash: &str) -> (Trust, Vec<u8>) {
        release::sign_test_manifest(manifest_payload(hash))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn update_verifies_and_atomically_replaces_executable() -> Result<()> {
        let old = b"#!/bin/sh\necho old\n";
        let new = b"#!/bin/sh\necho new\n";
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("magic");
        fs::write(&executable, old)?;
        let name = "linux-arm64";
        let (trust, manifest) = signed_manifest(&sha256(new));
        let payload = new.to_vec();
        let app = Router::new()
            .route(
                "/latest.signed.json",
                get(move || async move { manifest.clone() }),
            )
            .route(
                concat!(
                    "/releases/",
                    env!("CARGO_PKG_VERSION"),
                    "/magic-linux-arm64"
                ),
                get(move || async move { payload.clone() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            update(&base, &executable, name, &trust).await?,
            concat!("Updated magic to release ", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(fs::read(&executable)?, new);
        assert_eq!(
            update(&base, &executable, name, &trust).await?,
            concat!(
                "magic is already the latest version (",
                env!("CARGO_PKG_VERSION"),
                ")"
            )
        );
        server.abort();
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_releases_preserve_the_installed_executable() -> Result<()> {
        let old = b"#!/bin/sh\necho old\n";
        let (trust, valid) = signed_manifest(&sha256(b"expected binary"));
        let (_, wrong_key) = signed_manifest(&sha256(b"expected binary"));
        let (bad_executable_trust, bad_executable) = signed_manifest(&sha256(b"cannot execute"));
        let mut expired = manifest_payload(&sha256(b"expected binary"));
        expired["issued_at"] = json!(1);
        expired["expires_at"] = json!(2);
        let (expired_trust, expired) = release::sign_test_manifest(expired);
        let mut wrong_publisher = manifest_payload(&sha256(b"expected binary"));
        wrong_publisher["apple_team_id"] = json!("OTHER12345");
        let (wrong_publisher_trust, wrong_publisher) = release::sign_test_manifest(wrong_publisher);
        let mut downgrade = manifest_payload(&sha256(b"expected binary"));
        downgrade["release"] = json!("1.0.0");
        let (downgrade_trust, downgrade) = release::sign_test_manifest(downgrade);
        // A matching hash from an unsigned manifest or another signer cannot authorize execution.
        for (manifest, payload, trust, expected_error) in [
            (valid, "wrong binary", trust.clone(), "SHA-256"),
            (wrong_key, "expected binary", trust.clone(), "signature"),
            (expired, "expected binary", expired_trust, "expired"),
            (downgrade, "expected binary", downgrade_trust, "older"),
            (
                wrong_publisher,
                "expected binary",
                wrong_publisher_trust,
                "unexpected Apple publisher",
            ),
            (
                br#"{"release":"1.1.2","platforms":{"linux-arm64":"bad"}}"#.to_vec(),
                "",
                trust.clone(),
                "signed CLI release manifest",
            ),
            (
                b"invalid JSON".to_vec(),
                "",
                trust.clone(),
                "signed CLI release manifest",
            ),
            (
                bad_executable,
                "cannot execute",
                bad_executable_trust,
                "could not run",
            ),
        ] {
            let directory = tempfile::tempdir()?;
            let executable = directory.path().join("magic");
            fs::write(&executable, old)?;
            let app = Router::new()
                .route(
                    "/latest.signed.json",
                    get(move || async move { manifest.clone() }),
                )
                .route(
                    concat!(
                        "/releases/",
                        env!("CARGO_PKG_VERSION"),
                        "/magic-linux-arm64"
                    ),
                    get(move || async move { payload }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let error = update(&base, &executable, "linux-arm64", &trust)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected_error), "{error:#}");
            assert_eq!(fs::read(&executable)?, old);
            server.abort();
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_rejects_unsigned_binary_even_with_a_valid_release_manifest() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("magic");
        let marker = directory.path().join("executed");
        fs::write(&executable, b"old installation")?;
        let binary = format!("#!/bin/sh\ntouch '{}'\n", marker.display()).into_bytes();
        let (trust, manifest) = signed_manifest(&sha256(&binary));
        let app = Router::new()
            .route(
                "/latest.signed.json",
                get(move || async move { manifest.clone() }),
            )
            .route(
                concat!(
                    "/releases/",
                    env!("CARGO_PKG_VERSION"),
                    "/magic-darwin-arm64"
                ),
                get(move || async move { binary.clone() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(update(&base, &executable, "darwin-arm64", &trust)
            .await
            .is_err());
        assert!(!marker.exists());
        assert_eq!(fs::read(&executable)?, b"old installation");
        server.abort();
        Ok(())
    }
}
