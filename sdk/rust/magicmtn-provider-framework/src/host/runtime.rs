//! Runtime closures are built into packages; installation never installs global dependencies.
use super::{digest, ProcessEnvironment};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

pub const MAX_RUNTIME_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBundle {
    pub sha256: String,
    /// Environment values are paths relative to the unpacked runtime root.
    pub environment: BTreeMap<String, String>,
    pub check: RuntimeCheck,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCheck {
    pub executable: String,
    pub args: Vec<String>,
}
fn relative(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "invalid runtime path"
    );
    Ok(path)
}
impl RuntimeBundle {
    pub fn environment(&self, release: &Path) -> Result<BTreeMap<String, String>> {
        ensure!(self.environment.len() <= 16, "too many runtime settings");
        let release = release.canonicalize()?;
        self.environment
            .iter()
            .map(|(key, value)| {
                // Runtime selectors and TLS certificate paths are package-relative. PATH, HOME,
                // credentials and Python import paths stay controlled by the host.
                ensure!(
                    (key.starts_with("MAGICMTN_")
                        || matches!(
                            key.as_str(),
                            "SSL_CERT_FILE"
                                | "REQUESTS_CA_BUNDLE"
                                | "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH"
                        ))
                        && key.len() <= 96
                        && key
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                    "invalid runtime environment key"
                );
                let path = release.join("runtime").join(relative(value)?);
                ensure!(path.is_file(), "runtime file missing");
                Ok((
                    key.clone(),
                    path.to_str()
                        .context("runtime path is not UTF-8")?
                        .to_owned(),
                ))
            })
            .collect()
    }
}

pub async fn install(spec: &RuntimeBundle, bytes: &[u8], release: &Path) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= MAX_RUNTIME_BYTES && digest(bytes) == spec.sha256,
        "runtime checksum mismatch"
    );
    let root = release.join("runtime");
    std::fs::create_dir(&root)?;
    let root = root.canonicalize()?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut expanded = 0u64;
    let mut seen = HashSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        ensure!(
            entry.header().entry_type().is_file(),
            "runtime must contain regular files only"
        );
        let path: PathBuf = entry.path()?.into_owned();
        relative(path.to_str().context("invalid runtime path")?)?;
        ensure!(
            seen.len() < 50000 && seen.insert(path.clone()),
            "duplicate or excessive runtime entries"
        );
        expanded = expanded
            .checked_add(entry.size())
            .context("runtime size overflow")?;
        ensure!(
            expanded <= MAX_EXPANDED_BYTES,
            "runtime expands beyond its size limit"
        );
        let output = root.join(path);
        std::fs::create_dir_all(output.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        std::io::copy(&mut entry, &mut file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if entry.header().mode()? & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            std::fs::set_permissions(output, std::fs::Permissions::from_mode(mode))?;
        }
    }
    let environment = ProcessEnvironment {
        values: spec.environment(release)?,
    };
    let executable = root.join(relative(&spec.check.executable)?);
    ensure!(
        executable.is_file() && spec.check.args.len() <= 32,
        "invalid runtime check"
    );
    let scratch = tempfile::tempdir()?;
    let status = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(executable)
            .args(&spec.check.args)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", scratch.path())
            .env("TMPDIR", scratch.path())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .envs(environment.values)
            .current_dir(scratch.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("provider runtime check timed out")??;
    ensure!(status.success(), "provider runtime check failed");
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn archive(name: &str, body: &[u8], kind: tar::EntryType) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(kind);
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        tar.append(&header, body).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
    fn spec(bytes: &[u8]) -> RuntimeBundle {
        RuntimeBundle {
            sha256: digest(bytes),
            environment: [("MAGICMTN_FIXTURE_PYTHON".into(), "bin/python".into())].into(),
            check: RuntimeCheck {
                executable: "bin/python".into(),
                args: vec![],
            },
        }
    }
    #[test]
    fn runtime_checks_integrity_and_execution_without_inheriting_host_paths() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let bytes = archive("bin/python", b"#!/bin/sh\ntest -n \"$MAGICMTN_FIXTURE_PYTHON\"\ntest \"$PATH\" = /usr/local/bin:/usr/bin:/bin\n", tar::EntryType::Regular);
        let root = tempfile::tempdir().unwrap();
        install(&spec(&bytes), &bytes, root.path()).await.unwrap();
        let mut invalid = spec(&bytes);
        invalid.sha256 = "0".repeat(64);
        assert!(
            install(&invalid, &bytes, tempfile::tempdir().unwrap().path())
                .await
                .is_err()
        );
        let failed = archive(
            "bin/python",
            b"#!/bin/sh\nexit 7\n",
            tar::EntryType::Regular,
        );
        assert!(
            install(&spec(&failed), &failed, tempfile::tempdir().unwrap().path())
                .await
                .is_err()
        );
     });
    }
    #[test]
    fn runtime_rejects_traversal_links_and_environment_injection() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for (name, kind) in [
                    ("../outside", tar::EntryType::Regular),
                    ("/outside", tar::EntryType::Regular),
                    ("bin/python", tar::EntryType::Symlink),
                ] {
                    let bytes = archive(name, b"", kind);
                    assert!(
                        install(&spec(&bytes), &bytes, tempfile::tempdir().unwrap().path())
                            .await
                            .is_err()
                    );
                }
                let bytes = archive(
                    "bin/python",
                    b"#!/bin/sh\nexit 0\n",
                    tar::EntryType::Regular,
                );
                let mut invalid = spec(&bytes);
                invalid.environment = [("PATH".into(), "bin/python".into())].into();
                assert!(
                    install(&invalid, &bytes, tempfile::tempdir().unwrap().path())
                        .await
                        .is_err()
                );
            });
    }
}
