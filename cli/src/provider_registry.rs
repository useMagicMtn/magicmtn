//! Provider installation from the MagicMtn Registry or local packages.
use anyhow::{ensure, Context, Result};
use magicmtn_provider_framework::host::{Package, Registry};
use magicmtn_sdk::plugin::{CREDENTIAL_PROTOCOL_VERSION, PROTOCOL_VERSION};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::Path,
    time::Duration,
};

const ORIGIN: &str = "https://magicmtn-provider-registry-993499103295.s3.us-east-2.amazonaws.com";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    schema_version: u32,
    providers: Vec<Provider>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provider {
    name: String,
    description: String,
    version: String,
    platforms: BTreeMap<String, Artifact>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    sha256: String,
    executable_sha256: String,
}
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().next().unwrap().is_ascii_lowercase()
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn checksum(bytes: &[u8], expected: &str) -> Result<()> {
    ensure!(
        expected.len() == 64 && expected.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid Registry checksum"
    );
    ensure!(
        format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(expected),
        "downloaded provider failed SHA-256 verification"
    );
    Ok(())
}
async fn download(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    ensure!(
        response.content_length().is_none_or(|n| n <= limit as u64),
        "Registry response is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= limit.saturating_sub(bytes.len()),
            "Registry response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn select<'a>(
    index: &'a Index,
    name: &str,
    platform: &str,
) -> Result<(&'a Provider, &'a Artifact)> {
    ensure!(index.schema_version == 1, "unsupported Registry schema");
    let mut names = HashSet::new();
    for provider in &index.providers {
        ensure!(
            valid_name(&provider.name) && names.insert(&provider.name),
            "invalid or duplicate Registry name"
        );
        ensure!(
            !provider.description.is_empty(),
            "missing Registry description"
        );
        ensure!(
            !provider.version.is_empty()
                && provider
                    .version
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c)),
            "invalid Registry version"
        );
    }
    let provider = index
        .providers
        .iter()
        .find(|p| p.name == name)
        .context("provider is not available in the MagicMtn Registry")?;
    let artifact = provider.platforms.get(platform).context(
        "this provider has no Registry build for your operating system and architecture",
    )?;
    Ok((provider, artifact))
}
/// Check identity before Registry::install executes the adapter's describe RPC.
fn verify_package(bytes: &[u8], name: &str, version: &str, artifact: &Artifact) -> Result<()> {
    checksum(bytes, &artifact.sha256)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut package = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == std::path::Path::new("package.json") {
            ensure!(
                package.is_none() && entry.size() <= 65536 && entry.header().entry_type().is_file(),
                "invalid package manifest"
            );
            let mut data = Vec::new();
            entry.by_ref().take(65537).read_to_end(&mut data)?;
            package = Some(serde_json::from_slice::<Package>(&data)?);
        }
    }
    let package = package.context("provider package has no manifest")?;
    ensure!(
        package.name == name
            && package.version == version
            && package.sha256 == artifact.executable_sha256
            && package.os == std::env::consts::OS
            && package.arch == std::env::consts::ARCH
            && matches!(
                package.protocol_version,
                PROTOCOL_VERSION | CREDENTIAL_PROTOCOL_VERSION
            ),
        "provider package does not match its Registry identity"
    );
    Ok(())
}
pub(crate) async fn install(name: &str) -> Result<()> {
    ensure!(valid_name(name), "invalid provider name");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()?;
    let index: Index = serde_json::from_slice(
        &download(&client, &format!("{ORIGIN}/index.json"), 1024 * 1024).await?,
    )?;
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let (provider, artifact) = select(&index, name, &platform)?;
    let url = format!(
        "{ORIGIN}/providers/{name}/{}/{}.tar.gz",
        provider.version, platform
    );
    let bytes = download(&client, &url, 320 * 1024 * 1024).await.context(
        "downloading the current Registry release; retry installation if a release changed",
    )?;
    install_package(&bytes, Some((provider, artifact))).await
}

pub(crate) async fn install_file(file: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .context("opening provider package")?
        .take(320 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .context("reading provider package")?;
    ensure!(
        bytes.len() <= 320 * 1024 * 1024,
        "provider package exceeds 64 MiB"
    );
    install_package(&bytes, None).await
}

async fn install_package(bytes: &[u8], source: Option<(&Provider, &Artifact)>) -> Result<()> {
    let root = crate::configuration::directory()?.join("providers");
    let _lock = crate::configuration::lock_provider_store(&root)?;
    let installed = install_at(&root, bytes, source).await?;
    println!(
        "Installed {} {} ({}-{})",
        installed.package.name,
        installed.package.version,
        installed.package.os,
        installed.package.arch,
    );
    drop(_lock);
    Ok(())
}

async fn install_at(
    root: &Path,
    bytes: &[u8],
    source: Option<(&Provider, &Artifact)>,
) -> Result<magicmtn_provider_framework::host::Installed> {
    if let Some((provider, artifact)) = source {
        verify_package(bytes, &provider.name, &provider.version, artifact)?;
    }
    let registry = Registry::new(root.to_owned())?;
    let installed = registry.install(bytes).await?;
    let directory = root
        .join(&installed.package.name)
        .join(&installed.package.version);
    // An explicit local package is also a user-authorized installation. Its
    // receipt must not claim registry verification.
    let (origin, archive_sha256) = source
        .map(|(_, artifact)| (ORIGIN, artifact.sha256.clone()))
        .unwrap_or_else(|| ("local-package", format!("{:x}", Sha256::digest(bytes))));
    let mut receipt = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(
        &mut receipt,
        &serde_json::json!({"registry":origin,"pin":installed.pin(),"archive_sha256":archive_sha256}),
    )?;
    receipt.persist(directory.join("registry-origin.json"))?;
    crate::configuration::write_catalog(&registry, root)?;
    Ok(installed)
}

pub(crate) async fn configure(
    api: &crate::Api,
    name: &str,
    secret_string: Option<String>,
    secret_file: Option<&Path>,
    options: &[String],
) -> Result<()> {
    const MAX_BUNDLE_BYTES: u64 = 65_536;
    let root = crate::configuration::directory()?.join("providers");
    let installed = Registry::new(root)?.registry_release(name)?;
    ensure!(
        installed.manifest.credential.is_some(),
        "provider uses workload identity or has no credential fields"
    );
    let bytes = if let Some(path) = secret_file {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .context("opening provider credential file")?
            .take(MAX_BUNDLE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading provider credential file")?;
        bytes
    } else {
        secret_string
            .context("provide a JSON credential string or file")?
            .into_bytes()
    };
    ensure!(
        bytes.len() as u64 <= MAX_BUNDLE_BYTES,
        "provider credential exceeds the size limit"
    );
    let credential: magicmtn_sdk::plugin::CredentialValues = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid provider credential JSON"))?;
    let mut settings = BTreeMap::new();
    for option in options {
        let (key, value) = option
            .split_once('=')
            .context("provider option must be KEY=VALUE")?;
        ensure!(
            settings.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate provider option"
        );
    }
    if let Some(spec) = &installed.manifest.capabilities.setup {
        spec.validate_options(&settings)?;
    } else {
        ensure!(settings.is_empty(), "provider has no setup options");
    }
    let bytes =
        serde_json::to_vec(&serde_json::json!({"credential":credential,"options":settings}))?;
    let url = api.base.join(&format!("v1/providers/{name}/configure"))?;
    let response = api
        .authenticated(api.client.put(url))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await
        .context("could not reach the local MagicMtn server")?;
    if !response.status().is_success() {
        anyhow::bail!("provider configuration failed: {}", response.text().await?);
    }
    println!("Provider {name} ready.");
    Ok(())
}

pub(crate) fn list(format: crate::ShowFormat) -> Result<()> {
    let root = crate::configuration::directory()?.join("providers");
    let installed = Registry::new(root)?.list()?;
    match format {
        crate::ShowFormat::Json => println!("{}", serde_json::to_string_pretty(&installed)?),
        crate::ShowFormat::Human => {
            for release in installed {
                println!("{} {}", release.package.name, release.package.version);
            }
        }
    }
    Ok(())
}

pub(crate) async fn uninstall(name: &str) -> Result<()> {
    ensure!(valid_name(name), "invalid provider name");
    let directory = crate::configuration::directory()?;
    let root = directory.join("providers");
    let _lock = crate::configuration::lock_provider_store(&root)?;
    let registry = Registry::new(root.clone())?;
    let installed: Vec<_> = registry
        .list()?
        .into_iter()
        .filter(|release| release.package.name == name)
        .collect();
    if installed.is_empty() {
        crate::configuration::write_catalog(&registry, &root)?;
        println!("Provider {name} is not installed locally.");
        return Ok(());
    }
    let bindings: serde_json::Value = match std::fs::read(directory.join("local-connections.json"))
    {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    for (server, connection) in bindings.as_object().context("invalid local connections")? {
        let mut api = crate::Api::new(server)?;
        api.token = crate::authentication::Credentials::new()?.load(&api.base)?;
        let sandboxes = api.request(reqwest::Method::GET, "v1/sandboxes", None).await
            .with_context(|| format!("cannot check local sandbox references on {server}; sign in to that server before uninstalling"))?;
        ensure!(!sandboxes.as_array().context("invalid sandbox list")?.iter().any(|sandbox| {
            sandbox["metadata"]["_magicmtn_connection"]["connection_id"] == *connection
                && sandbox["metadata"]["_magicmtn_connection"]["pin"]["name"] == name
                && !matches!(sandbox["status"].as_str(), Some("ended" | "failed"))
        }), "provider {name} is still required by a local sandbox; finish sandbox cleanup before uninstalling");
    }
    remove_local(&registry, &root, name).await?;
    println!("Uninstalled {name} ({} local version(s)).", installed.len());
    Ok(())
}

async fn remove_local(registry: &Registry, root: &std::path::Path, name: &str) -> Result<()> {
    ensure!(valid_name(name), "invalid provider name");
    let _gate = registry.gate.lock().await;
    for release in registry
        .list()?
        .into_iter()
        .filter(|release| release.package.name == name)
    {
        registry.remove(name, &release.package.version).await?;
    }
    crate::configuration::write_catalog(registry, root)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Index {
        serde_json::from_value(serde_json::json!({"schema_version":1,"providers":[{"name":"apple-container","description":"Local sandboxes","version":"0.1.2","platforms":{"macos-aarch64":{"sha256":"a".repeat(64),"executable_sha256":"b".repeat(64)}}}]})).unwrap()
    }
    #[test]
    fn authority_selects_unique_names_and_supported_platforms() {
        let mut index = fixture();
        assert!(select(&index, "apple-container", "macos-aarch64").is_ok());
        assert!(select(&index, "private-provider", "macos-aarch64").is_err());
        assert!(select(&index, "apple-container", "linux-x86_64").is_err());
        index.providers.push(fixture().providers.remove(0));
        assert!(select(&index, "apple-container", "macos-aarch64").is_err());
    }
    #[test]
    fn checksums_and_names_fail_closed() {
        assert!(checksum(b"changed", &format!("{:x}", Sha256::digest(b"original"))).is_err());
        for name in ["../provider", "https://example.com", "Apple", "", "a/b"] {
            assert!(!valid_name(name));
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installation_records_local_and_verified_registry_origins_separately() {
        use magicmtn_sdk::plugin::{Capabilities, ProviderManifest};
        use serde_json::json;

        let manifest = ProviderManifest {
            credential: None,
            manifest_version: 1,
            provider_version: "1.0.0".into(),
            capabilities: Capabilities::sandbox(vec![]),
        };
        let description = json!({
            "id": 1, "protocol_version": 1, "provider": "example",
            "outcome": {"ok": {"kind": "description", "value": manifest}}
        });
        let executable = format!(
            "#!/bin/sh\n\
             test -n \"$HOME\" && test \"$HOME\" = \"$TMPDIR\" && test . -ef \"$HOME\" || exit 1\n\
             IFS= read -r frame || exit 1\n\
             printf '%s\\n' '{description}'\n\
             IFS= read -r frame\n"
        );
        let package = Package {
            runtime: None,
            name: "example".into(),
            version: "1.0.0".into(),
            protocol_version: 1,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            sha256: format!("{:x}", Sha256::digest(executable.as_bytes())),
        };
        let metadata = serde_json::to_vec(&package).unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in [
            ("package.json", metadata.as_slice()),
            ("provider", executable.as_bytes()),
            ("LICENSE", b"fixture".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, contents).unwrap();
        }
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let provider = Provider {
            name: package.name.clone(),
            description: "fixture".into(),
            version: package.version.clone(),
            platforms: BTreeMap::new(),
        };
        let artifact = Artifact {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            executable_sha256: package.sha256.clone(),
        };
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let release = root.join("example/1.0.0");
        let receipt = release.join("registry-origin.json");

        install_at(root, &bytes, None).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&receipt).unwrap()).unwrap()
                ["registry"],
            "local-package"
        );
        let registry = Registry::new(root.to_owned()).unwrap();
        assert!(registry.registry_release("example").is_ok());
        let path = release.join("provider");
        std::fs::write(&path, b"changed").unwrap();
        assert!(install_at(root, &bytes, Some((&provider, &artifact)))
            .await
            .is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&receipt).unwrap()).unwrap()
                ["registry"],
            "local-package"
        );
        std::fs::write(&path, executable).unwrap();
        let installed = install_at(root, &bytes, Some((&provider, &artifact)))
            .await
            .unwrap();
        assert_eq!(
            registry.registry_release("example").unwrap().pin(),
            installed.pin()
        );
        for invalid in ["provider-example", "Example", "../example"] {
            assert!(registry.registry_release(invalid).is_err());
        }
        let recorded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        assert_eq!(
            recorded,
            json!({
                "registry": ORIGIN, "pin": installed.pin(), "archive_sha256": artifact.sha256
            })
        );
    }

    #[tokio::test]
    async fn uninstall_removes_all_versions_and_preserves_other_providers() {
        use magicmtn_sdk::plugin::{Capabilities, ProviderManifest};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let registry = Registry::new(root.to_owned()).unwrap();
        for (name, version) in [
            ("example", "1.0.0"),
            ("example", "2.0.0"),
            ("keep", "1.0.0"),
        ] {
            let path = root.join(name).join(version);
            std::fs::create_dir_all(&path).unwrap();
            let package = Package {
                runtime: None,
                name: name.into(),
                version: version.into(),
                protocol_version: 1,
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                sha256: format!("{:x}", Sha256::digest(b"unused")),
            };
            let manifest = ProviderManifest {
                credential: None,
                manifest_version: 1,
                provider_version: version.into(),
                capabilities: Capabilities::sandbox(vec![]),
            };
            std::fs::write(
                path.join("package.json"),
                serde_json::to_vec(&package).unwrap(),
            )
            .unwrap();
            std::fs::write(
                path.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(path.join("provider"), b"unused").unwrap();
            std::fs::write(root.join(name).join("active"), version).unwrap();
        }
        remove_local(&registry, root, "example").await.unwrap();
        assert!(!root.join("example/1.0.0").exists());
        assert!(!root.join("example/2.0.0").exists());
        assert!(!root.join("example/active").exists());
        assert!(root.join("keep/1.0.0/provider").exists());
        let catalog: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("catalog.json")).unwrap()).unwrap();
        assert_eq!(catalog.as_array().unwrap().len(), 1);
        assert_eq!(catalog[0]["package"]["name"], "keep");
        remove_local(&registry, root, "example").await.unwrap();
    }

    #[test]
    fn modification_is_blocked_while_runner_holds_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".lock");
        let runner = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        runner.try_lock_shared().unwrap();
        assert!(crate::configuration::lock_provider_store(directory.path()).is_err());
        runner.unlock().unwrap();
        assert!(crate::configuration::lock_provider_store(directory.path()).is_ok());
    }
}
