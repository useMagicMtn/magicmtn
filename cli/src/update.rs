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

async fn update(base: &str, executable: &Path, platform: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let bytes = get(&client, &format!("{base}/latest.json"), 32 * 1024).await?;
    #[derive(serde::Deserialize)]
    struct Manifest {
        release: String,
        platforms: std::collections::BTreeMap<String, String>,
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).context("invalid release manifest")?;
    let release = &manifest.release;
    anyhow::ensure!(
        !release.is_empty()
            && release.len() <= 100
            && release
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.'),
        "invalid release identifier"
    );
    let expected = manifest
        .platforms
        .get(platform)
        .context("no release for this platform")?;
    anyhow::ensure!(
        expected.len() == 64 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid release SHA-256"
    );
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
    println!("{}", update(DOWNLOADS, &executable, platform()?).await?);
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

    fn checksum_manifest(hash: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "release": "1.0.0", "platforms": {"linux-arm64": hash}
        }))
        .unwrap()
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
        let manifest = checksum_manifest(&sha256(new));
        let payload = new.to_vec();
        let app = Router::new()
            .route("/latest.json", get(move || async move { manifest.clone() }))
            .route(
                "/releases/1.0.0/magic-linux-arm64",
                get(move || async move { payload.clone() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            update(&base, &executable, name).await?,
            "Updated magic to release 1.0.0"
        );
        assert_eq!(fs::read(&executable)?, new);
        assert_eq!(
            update(&base, &executable, name).await?,
            "magic is already the latest version (1.0.0)"
        );
        server.abort();
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejected_releases_preserve_the_installed_executable() -> Result<()> {
        let old = b"#!/bin/sh\necho old\n";
        for (manifest, payload) in [
            (
                checksum_manifest(&sha256(b"expected binary")),
                "wrong binary",
            ),
            (
                br#"{"release":"../escape","platforms":{"linux-arm64":"bad"}}"#.to_vec(),
                "",
            ),
            (checksum_manifest("invalid hash"), ""),
            (br#"{"release":"1.0.0","platforms":{}}"#.to_vec(), ""),
            (b"invalid JSON".to_vec(), ""),
            (
                checksum_manifest(&sha256(b"cannot execute")),
                "cannot execute",
            ),
        ] {
            let directory = tempfile::tempdir()?;
            let executable = directory.path().join("magic");
            fs::write(&executable, old)?;
            let app = Router::new()
                .route("/latest.json", get(move || async move { manifest.clone() }))
                .route(
                    "/releases/1.0.0/magic-linux-arm64",
                    get(move || async move { payload }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            assert!(update(&base, &executable, "linux-arm64").await.is_err());
            assert_eq!(fs::read(&executable)?, old);
            server.abort();
        }
        Ok(())
    }
}
