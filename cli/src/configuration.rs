//! User preferences are deliberately separate from server-scoped runner registration.
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Config {
    // Accepted for compatibility; server selection is owned by mode::Selection.
    #[serde(rename = "local-sandbox")]
    _local_sandbox: bool,
    error_reporting: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            _local_sandbox: false,
            error_reporting: false,
        }
    }
}
pub(crate) fn error_reporting() -> Result<bool> {
    Ok(read(&directory()?.join("config"))?.error_reporting)
}
pub(crate) fn directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MAGICMTN_CONFIG_DIRECTORY") {
        return Ok(path.into());
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
            .join(".config/magicmtn"),
    )
}
fn read(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}
pub(crate) fn creation_body(body: Value) -> Result<Value> {
    creation_body_at(&directory()?, body)
}

fn creation_body_at(root: &Path, mut body: Value) -> Result<Value> {
    let config = read(&root.join("config"))?;
    body["error_reporting"] = json!(config.error_reporting);
    Ok(body)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn error_reporting_requires_explicit_boolean_opt_in() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        assert!(!read(&path).unwrap().error_reporting);
        for (text, expected) in [
            ("", false),
            ("error-reporting = true", true),
            ("local-sandbox = true", false),
            ("error-reporting = false", false),
            ("local-sandbox = true\nerror-reporting = false", false),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&path).unwrap().error_reporting, expected);
        }
        fs::write(&path, "error-reporting = 'false'").unwrap();
        assert!(read(&path).is_err());
    }

    #[test]
    fn legacy_local_preference_does_not_route_cloud_creation_to_a_local_runner() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("config"),
            "local-sandbox = true\nerror-reporting = false",
        )
        .unwrap();
        let body = json!({"model_id": "model", "idempotency_key": "request"});
        let expected =
            json!({"model_id": "model", "idempotency_key": "request", "error_reporting": false});
        assert_eq!(
            creation_body_at(root.path(), body.clone()).unwrap(),
            expected
        );

        // Even an old connection file must not override the selected Cloud mode.
        fs::write(root.path().join("local-connections.json"), "invalid JSON").unwrap();
        assert_eq!(creation_body_at(root.path(), body).unwrap(), expected);
    }

    #[test]
    fn config_is_opt_in_and_strict() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        assert!(!read(&path).unwrap()._local_sandbox);
        for (text, expected) in [
            ("", false),
            ("local-sandbox = false", false),
            ("local-sandbox = true", true),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&path).unwrap()._local_sandbox, expected);
        }
        for text in [
            "local-sandbox: true",
            "local-sandbox = 'true'",
            "local_sandbox = true",
        ] {
            fs::write(&path, text).unwrap();
            assert!(read(&path).is_err());
        }
    }
}

pub(crate) fn write_catalog(
    registry: &magicmtn_provider_framework::host::Registry,
    root: &Path,
) -> Result<()> {
    let releases: Vec<_> = registry.list()?.into_iter().map(|installed| {
        let executable = root.join(&installed.package.name).join(&installed.package.version).join("provider");
        let directory = executable.parent().unwrap();
        let runtime_environment = installed.package.runtime.as_ref().map(|spec| spec.environment(directory)).transpose()?.unwrap_or_default();
        Ok(json!({"package":installed.package,"manifest":installed.manifest,"active":installed.active,"executable":executable,"runtime_environment":runtime_environment}))
    }).collect::<Result<Vec<_>>>()?;
    let catalog = root.join("catalog.json");
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut file, &releases)?;
    file.persist(&catalog)?;
    Ok(())
}

/// Keep install/remove operations separate from running provider processes.
pub(crate) fn lock_provider_store(root: &Path) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    lock.try_lock().context("provider store is in use; stop the local runner before installing or uninstalling providers")?;
    Ok(lock)
}
