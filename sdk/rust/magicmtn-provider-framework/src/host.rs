//! Private, versioned installations and a reusable host for SDK RPC providers.
use super::MAX_MESSAGE_BYTES;
use anyhow::{bail, ensure, Context, Result};
use magicmtn_sdk::plugin::{
    CredentialValues, ErrorCode, Operation, Outcome, ProviderError, ProviderManifest,
    ProviderStartup, Request, Response, Value, CREDENTIAL_PROTOCOL_VERSION, MANIFEST_VERSION,
    PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

mod credentials;
pub use credentials::{CredentialAdmission, CredentialRef, CredentialUpdate, PreparedCredential};

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn valid_version(version: &str) -> bool {
    version.len() <= 100 && semver::Version::parse(version).is_ok()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub name: String,
    pub version: String,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub protocol_version: u32,
    pub os: String,
    pub arch: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub package: Package,
    pub manifest: ProviderManifest,
    pub active: bool,
}

/// Build-produced catalog. Executables and dependency closure live in the image.
/// Metadata-only entries describe releases executed by a connected local runner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    #[serde(flatten)]
    pub installed: Installed,
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub configuration_keys: Vec<String>,
    /// Build-time interpreter/library paths; credentials are always runtime data.
    #[serde(default)]
    pub runtime_environment: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
pub struct ProcessEnvironment {
    /// Explicit environment for one connection. Never log or serialize this.
    pub values: BTreeMap<String, String>,
}
impl Installed {
    pub fn pin(&self) -> Pin {
        Pin {
            name: self.package.name.clone(),
            version: self.package.version.clone(),
            sha256: self.package.sha256.clone(),
        }
    }
}

fn retained_chat_input(operation: &mut Operation) -> Result<()> {
    let Operation::Infer { request } = operation else {
        return Ok(());
    };
    if request.task != "chat" {
        return Ok(());
    }
    request.input = magicmtn_sdk::chat::normalize_input(&request.input)?;
    for message in request.input["messages"].as_array_mut().unwrap() {
        if let Some(calls) = message
            .get_mut("tool_calls")
            .and_then(serde_json::Value::as_array_mut)
        {
            for call in calls {
                call["function"]["arguments"] =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap())?;
                call.as_object_mut().unwrap().remove("id");
                call.as_object_mut().unwrap().remove("type");
            }
        }
        if message["role"] == "tool" {
            if let Some(name) = message.as_object_mut().unwrap().remove("name") {
                message["tool_name"] = name;
            }
            message.as_object_mut().unwrap().remove("tool_call_id");
        }
    }
    Ok(())
}

/// Fixed, non-secret AWS Nitro readiness errors that establish that no command was submitted.
/// All other credentialed provider messages remain redacted.
pub fn nitro_startup_error(provider: &str, error: &ProviderError) -> Option<&'static str> {
    if provider != "aws"
        || error.retryable
        || !matches!(
            error.code,
            ErrorCode::InvalidRequest | ErrorCode::UpstreamUnavailable
        )
    {
        return None;
    }
    [
        "Nitro sandbox is not ready",
        "Nitro sandbox is waiting for node capacity or scheduling constraints",
        "Nitro sandbox image could not be pulled",
        "Nitro sandbox lifetime expired",
        "Nitro sandbox has finished",
        "Nitro sandbox is terminating",
        "Nitro sandbox was removed",
        "Nitro Job identity changed",
        "unexpected Nitro sandbox pod ownership or runtime",
        "Nitro EKS readiness read was not authorized",
        "Nitro EKS readiness read failed",
        "Nitro EKS readiness watch failed",
        "Nitro EKS response too large",
        "invalid Nitro EKS response",
        "Nitro EKS watch event too large",
        "invalid Nitro EKS watch event",
        "incomplete Nitro EKS pod list",
        "missing Nitro EKS pod resource version",
    ]
    .into_iter()
    .find(|message| *message == error.message)
}

struct Connection {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    protocol_version: u32,
}
impl Connection {
    async fn exchange(&mut self, name: &str, mut operation: Operation) -> Result<Value> {
        if self.protocol_version == 3 {
            retained_chat_input(&mut operation)?;
        }
        let id = self.next;
        self.next += 1;
        let bytes = serde_json::to_vec(&Request {
            id,
            protocol_version: self.protocol_version,
            provider: name.into(),
            operation,
        })?;
        self.exchange_frame(name, id, bytes).await
    }

    async fn startup(
        &mut self,
        name: &str,
        credential: Option<&CredentialValues>,
        verify_account: bool,
    ) -> Result<Value> {
        if self.protocol_version != CREDENTIAL_PROTOCOL_VERSION {
            return self.exchange(name, Operation::Describe).await;
        }
        let id = self.next;
        self.next += 1;
        let bytes = serde_json::to_vec(&ProviderStartup {
            id,
            protocol_version: self.protocol_version,
            provider: name,
            credential,
            verify_account,
        })?;
        self.exchange_frame(name, id, bytes).await
    }

    async fn exchange_frame(&mut self, name: &str, id: u64, bytes: Vec<u8>) -> Result<Value> {
        let mut bytes = zeroize::Zeroizing::new(bytes);
        ensure!(
            (bytes.len() as u64) < MAX_MESSAGE_BYTES,
            "RPC request exceeds message limit"
        );
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        let mut response = Vec::new();
        (&mut self.output)
            .take(MAX_MESSAGE_BYTES + 1)
            .read_until(b'\n', &mut response)
            .await?;
        ensure!(
            response.len() as u64 <= MAX_MESSAGE_BYTES && response.ends_with(b"\n"),
            "provider closed or exceeded RPC frame limit"
        );
        let response: Response =
            serde_json::from_slice(&response).context("invalid provider RPC response")?;
        ensure!(
            response.id == id
                && response.provider == name
                && response.protocol_version == self.protocol_version,
            "provider RPC identity mismatch"
        );
        match response.outcome {
            Outcome::Ok(value) => Ok(value),
            Outcome::Error(mut error) => {
                if self.protocol_version == CREDENTIAL_PROTOCOL_VERSION {
                    error.message = nitro_startup_error(name, &error)
                        .unwrap_or("provider operation failed")
                        .into();
                }
                Err(error.into())
            }
        }
    }
}
pub struct Client {
    name: String,
    connection: Mutex<Option<Connection>>,
    pub manifest: ProviderManifest,
    _scratch: Option<tempfile::TempDir>,
}
impl Client {
    pub async fn start(path: &Path, name: &str) -> Result<Self> {
        Self::start_installed(path, name, PROTOCOL_VERSION).await
    }
    async fn start_installed(path: &Path, name: &str, protocol_version: u32) -> Result<Self> {
        Self::start_configured(path, name, protocol_version, None).await
    }
    async fn start_configured(
        path: &Path,
        name: &str,
        protocol_version: u32,
        environment: Option<&ProcessEnvironment>,
    ) -> Result<Self> {
        Self::start_with_credential(path, name, protocol_version, environment, None).await
    }

    async fn start_with_credential(
        path: &Path,
        name: &str,
        protocol_version: u32,
        environment: Option<&ProcessEnvironment>,
        credential: Option<&CredentialValues>,
    ) -> Result<Self> {
        let (mut connection, scratch) = Self::spawn(path, protocol_version, environment)?;
        let value = tokio::time::timeout(
            Duration::from_secs(10),
            connection.startup(name, credential, false),
        )
        .await
        .context("provider handshake timed out")??;
        let Value::Description(manifest) = value else {
            bail!("provider did not return a manifest")
        };
        validate_manifest(protocol_version, &manifest)?;
        Ok(Self {
            name: name.into(),
            connection: Mutex::new(Some(connection)),
            manifest,
            _scratch: scratch,
        })
    }

    fn spawn(
        path: &Path,
        protocol_version: u32,
        environment: Option<&ProcessEnvironment>,
    ) -> Result<(Connection, Option<tempfile::TempDir>)> {
        // Retained private installations predate the public v1 numbering.
        ensure!(
            matches!(
                protocol_version,
                PROTOCOL_VERSION | CREDENTIAL_PROTOCOL_VERSION | 3
            ),
            "unsupported installed provider protocol"
        );
        let mut command = Command::new(path);
        let scratch = if let Some(environment) = environment {
            let scratch = tempfile::tempdir()?;
            command
                .current_dir(scratch.path())
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", scratch.path())
                .env("TMPDIR", scratch.path())
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .envs(&environment.values);
            Some(scratch)
        } else {
            None
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if protocol_version == CREDENTIAL_PROTOCOL_VERSION {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .kill_on_drop(true)
            .spawn()
            .context("starting installed provider")?;
        let connection = Connection {
            input: child.stdin.take().context("missing stdin")?,
            output: BufReader::new(child.stdout.take().context("missing stdout")?),
            child,
            next: 1,
            protocol_version,
        };
        Ok((connection, scratch))
    }
    pub async fn call(&self, operation: Operation, timeout: Duration) -> Result<Value> {
        ensure!(
            self.manifest.capabilities.supports(&operation),
            "provider does not support this operation"
        );
        let mut state = self.connection.lock().await;
        let connection = state
            .as_mut()
            .context("provider connection failed; operation was not replayed")?;
        let result =
            tokio::time::timeout(timeout, connection.exchange(&self.name, operation)).await;
        // Any ambiguous failure kills this connection. Never retry a side effect.
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                if error
                    .downcast_ref::<magicmtn_sdk::plugin::ProviderError>()
                    .is_none()
                {
                    *state = None;
                }
                Err(error)
            }
            Err(error) => {
                *state = None;
                Err(error.into())
            }
        }
    }
    pub async fn shutdown(&self) {
        let mut state = self.connection.lock().await;
        if let Some(connection) = state.as_mut() {
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                connection.exchange(&self.name, Operation::Shutdown).await?;
                connection.child.wait().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
        }
        *state = None;
    }
}

fn validate_manifest(protocol: u32, manifest: &ProviderManifest) -> Result<()> {
    ensure!(
        manifest.manifest_version == MANIFEST_VERSION && valid_version(&manifest.provider_version),
        "unsupported provider manifest"
    );
    ensure!(
        manifest.credential.is_some() == (protocol == CREDENTIAL_PROTOCOL_VERSION),
        "provider credential protocol mismatch"
    );
    if let Some(spec) = &manifest.credential {
        spec.validate()?;
    }
    Ok(())
}

type Clients = HashMap<(String, String, String, String), Arc<Client>>;

#[derive(Clone)]
pub struct Registry {
    root: PathBuf,
    catalog: Option<Arc<Vec<Release>>>,
    pub gate: Arc<Mutex<()>>,
    clients: Arc<Mutex<Clients>>,
    credentials: Arc<credentials::Credentials>,
}
impl Registry {
    pub fn new(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            root,
            catalog: None,
            gate: Arc::new(Mutex::new(())),
            clients: Arc::new(Mutex::new(HashMap::new())),
            credentials: Default::default(),
        })
    }
    pub fn from_catalog(path: &Path) -> Result<Self> {
        let releases: Vec<Release> = serde_json::from_slice(&std::fs::read(path)?)?;
        Self::from_releases(releases)
    }
    pub fn from_releases(releases: Vec<Release>) -> Result<Self> {
        let mut identities = std::collections::HashSet::new();
        let mut active = std::collections::HashSet::new();
        for release in &releases {
            ensure!(
                release.runtime_environment.keys().all(|key| [
                    "PATH",
                    "PYTHONPATH",
                    "PYTHONHOME",
                    "SSL_CERT_FILE",
                    "SSL_CERT_DIR"
                ]
                .contains(&key.as_str())
                    || (key.starts_with("MAGICMTN_") && key.ends_with("_PYTHON"))),
                "catalog runtime environment may only configure dependency paths"
            );
            let p = &release.installed.package;
            ensure!(
                valid_name(&p.name)
                    && valid_version(&p.version)
                    && matches!(
                        p.protocol_version,
                        PROTOCOL_VERSION | CREDENTIAL_PROTOCOL_VERSION | 3
                    ),
                "invalid catalog release"
            );
            ensure!(
                p.sha256.len() == 64
                    && p.sha256
                        .bytes()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "invalid release digest"
            );
            ensure!(
                release.installed.manifest.provider_version == p.version
                    && release.installed.manifest.manifest_version == MANIFEST_VERSION,
                "catalog manifest version mismatch"
            );
            validate_manifest(p.protocol_version, &release.installed.manifest)?;
            ensure!(
                identities.insert((&p.name, &p.version, &p.os, &p.arch)),
                "duplicate catalog release"
            );
            if release.installed.active {
                ensure!(
                    active.insert((&p.name, &p.os, &p.arch)),
                    "multiple default releases"
                );
            }
            if let Some(path) = &release.executable {
                ensure!(
                    path.is_absolute()
                        && p.os == std::env::consts::OS
                        && p.arch == std::env::consts::ARCH,
                    "catalog executable platform/path mismatch"
                );
                ensure!(
                    digest(&std::fs::read(path)?) == p.sha256,
                    "catalog executable digest mismatch"
                );
            }
        }
        Ok(Self {
            root: PathBuf::new(),
            catalog: Some(Arc::new(releases)),
            gate: Arc::new(Mutex::new(())),
            clients: Arc::new(Mutex::new(HashMap::new())),
            credentials: Default::default(),
        })
    }
    pub fn immutable(&self) -> bool {
        self.catalog.is_some()
    }
    pub fn executes_on_host(&self, pin: &Pin) -> bool {
        self.catalog.as_ref().is_none_or(|c| {
            c.iter()
                .any(|r| r.installed.pin() == *pin && r.executable.is_some())
        })
    }
    pub fn configuration_keys(&self, name: &str, version: &str) -> Vec<String> {
        self.catalog
            .as_ref()
            .and_then(|c| {
                c.iter().find(|r| {
                    r.installed.package.name == name && r.installed.package.version == version
                })
            })
            .map(|r| r.configuration_keys.clone())
            .unwrap_or_default()
    }
    pub fn default_directory() -> Result<PathBuf> {
        if let Some(root) = std::env::var_os("MAGICMTN_PROVIDER_DIR") {
            return Ok(PathBuf::from(root));
        }
        let data = match std::env::var_os("XDG_DATA_HOME") {
            Some(data) => PathBuf::from(data),
            None => PathBuf::from(
                std::env::var_os("HOME").context("HOME is required for provider storage")?,
            )
            .join(".local/share"),
        };
        Ok(data.join("magicmtn/providers"))
    }
    fn directory(&self, name: &str, version: &str) -> Result<PathBuf> {
        ensure!(
            valid_name(name) && valid_version(version),
            "invalid provider name/version"
        );
        Ok(self.root.join(name).join(version))
    }
    pub fn installed(&self, name: &str, version: &str) -> Result<Installed> {
        if let Some(catalog) = &self.catalog {
            return catalog
                .iter()
                .filter(|r| {
                    r.installed.package.name == name && r.installed.package.version == version
                })
                .min_by_key(|r| if r.executable.is_some() { 0 } else { 1 })
                .map(|r| r.installed.clone())
                .context("provider release not in deployment catalog");
        }
        let directory = self.directory(name, version)?;
        let package: Package =
            serde_json::from_slice(&std::fs::read(directory.join("package.json"))?)?;
        let manifest: ProviderManifest =
            serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
        ensure!(
            package.name == name
                && package.version == version
                && manifest.provider_version == version,
            "installed provider metadata mismatch"
        );
        validate_manifest(package.protocol_version, &manifest)?;
        let active = std::fs::read_to_string(self.root.join(name).join("active"))
            .is_ok_and(|v| v == version);
        Ok(Installed {
            package,
            manifest,
            active,
        })
    }
    pub fn installed_pin(&self, pin: &Pin) -> Result<Installed> {
        if let Some(catalog) = &self.catalog {
            return catalog
                .iter()
                .find(|r| r.installed.pin() == *pin)
                .map(|r| r.installed.clone())
                .context("pinned provider release not in deployment catalog");
        }
        let installed = self.installed(&pin.name, &pin.version)?;
        ensure!(installed.pin() == *pin, "provider digest differs");
        Ok(installed)
    }
    pub fn active(&self, name: &str) -> Result<Installed> {
        if let Some(catalog) = &self.catalog {
            return catalog
                .iter()
                .filter(|r| r.installed.package.name == name && r.installed.active)
                .min_by_key(|r| if r.executable.is_some() { 0 } else { 1 })
                .map(|r| r.installed.clone())
                .context("provider has no default release");
        }
        ensure!(valid_name(name), "invalid provider name");
        let version = std::fs::read_to_string(self.root.join(name).join("active"))
            .context("provider is not installed/active")?;
        self.installed(name, &version)
    }
    /// Read installation provenance without consulting the registry or running a package.
    pub fn registry_release(&self, name: &str) -> Result<Installed> {
        #[derive(Deserialize)]
        struct Origin {
            registry: String,
            pin: Pin,
            archive_sha256: String,
        }
        // Validate before using the caller's name as a path component.
        ensure!(valid_name(name), "invalid provider name");
        let release = self
            .active(name)
            .with_context(|| format!("no installed provider named '{name}'"))?;
        let directory = self.directory(name, &release.package.version)?;
        let origin: Origin = serde_json::from_slice(
            &std::fs::read(directory.join("registry-origin.json")).context(
                "install this provider from the registry before configuring credentials",
            )?,
        )?;
        ensure!(
            !origin.registry.is_empty()
                && origin.pin == release.pin()
                && origin.archive_sha256.len() == 64
                && origin
                    .archive_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "installed provider differs from its registry identity"
        );
        Ok(release)
    }

    pub fn list(&self) -> Result<Vec<Installed>> {
        if let Some(catalog) = &self.catalog {
            return Ok(catalog.iter().map(|r| r.installed.clone()).collect());
        }
        let mut out = Vec::new();
        for name in std::fs::read_dir(&self.root)? {
            let name = name?;
            if !name.file_type()?.is_dir() {
                continue;
            }
            let name = name.file_name().to_string_lossy().into_owned();
            if !valid_name(&name) {
                continue;
            }
            for version in std::fs::read_dir(self.root.join(&name))? {
                let version = version?;
                if version.file_type()?.is_dir() {
                    let version = version.file_name().to_string_lossy().into_owned();
                    if valid_version(&version) {
                        out.push(self.installed(&name, &version)?);
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            (&a.package.name, &a.package.version).cmp(&(&b.package.name, &b.package.version))
        });
        Ok(out)
    }
    pub async fn install(&self, bytes: &[u8]) -> Result<Installed> {
        ensure!(
            !self.immutable(),
            "provider releases are immutable deployment content"
        );
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        let mut package = None;
        let mut executable = None;
        let mut license = None;
        let mut count = 0;
        for entry in archive.entries()? {
            count += 1;
            ensure!(count <= 3, "too many package entries");
            let mut entry = entry?;
            ensure!(
                entry.header().entry_type().is_file(),
                "package must contain regular files only"
            );
            let path = entry.path()?.to_string_lossy().into_owned();
            ensure!(
                ["package.json", "provider", "LICENSE"].contains(&path.as_str()),
                "unexpected package entry"
            );
            let limit = if path == "provider" {
                64 * 1024 * 1024
            } else {
                65536
            };
            ensure!(entry.size() <= limit, "package entry is too large");
            let mut body = Vec::new();
            entry.by_ref().take(limit + 1).read_to_end(&mut body)?;
            ensure!(body.len() as u64 <= limit, "package entry is too large");
            match path.as_str() {
                "package.json" => {
                    ensure!(package.is_none(), "duplicate package manifest");
                    package = Some(serde_json::from_slice::<Package>(&body)?);
                }
                "provider" => {
                    ensure!(executable.is_none(), "duplicate provider");
                    executable = Some(body);
                }
                _ => {
                    ensure!(license.is_none(), "duplicate license");
                    license = Some(body);
                }
            }
        }
        let license = license.context("missing LICENSE")?;
        let package = package.context("missing package.json")?;
        let bytes = executable.context("missing provider executable")?;
        ensure!(
            valid_name(&package.name) && valid_version(&package.version),
            "invalid package identity"
        );
        ensure!(
            package.os == std::env::consts::OS && package.arch == std::env::consts::ARCH,
            "provider platform/protocol is incompatible with this host"
        );
        ensure!(
            digest(&bytes) == package.sha256,
            "provider checksum mismatch"
        );
        let directory = self.directory(&package.name, &package.version)?;
        let _guard = self.gate.lock().await;
        if directory.exists() {
            let current = self.installed(&package.name, &package.version)?;
            ensure!(
                current.package == package,
                "cannot replace an installed provider version"
            );
            ensure!(
                digest(&std::fs::read(directory.join("provider"))?) == package.sha256,
                "installed provider checksum changed"
            );
            self.activate(&package.name, &package.version)?;
            return self.installed(&package.name, &package.version);
        }
        // Re-enrollment of an unchanged retained package uses its existing
        // implementation. Fresh installations must use the public SDK protocol.
        ensure!(
            matches!(
                package.protocol_version,
                PROTOCOL_VERSION | CREDENTIAL_PROTOCOL_VERSION
            ),
            "provider platform/protocol is incompatible with this host"
        );
        let staged = tempfile::tempdir_in(&self.root)?;
        let path = staged.path().join("provider");
        std::fs::write(&path, bytes)?;
        std::fs::write(staged.path().join("LICENSE"), license)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        // Discovering capabilities runs package code before any connection has
        // authorized credentials. Use the same clean environment as dispatch.
        let client = Client::start_configured(
            &path.canonicalize()?,
            &package.name,
            package.protocol_version,
            Some(&ProcessEnvironment::default()),
        )
        .await?;
        ensure!(
            client.manifest.provider_version == package.version,
            "package version differs from handshake"
        );
        let manifest = client.manifest.clone();
        client.shutdown().await;
        std::fs::write(
            staged.path().join("package.json"),
            serde_json::to_vec_pretty(&package)?,
        )?;
        std::fs::write(
            staged.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        std::fs::create_dir_all(
            directory
                .parent()
                .context("invalid installation directory")?,
        )?;
        std::fs::rename(staged.path(), &directory)?;
        self.activate(&package.name, &package.version)?;
        self.installed(&package.name, &package.version)
    }
    fn activate(&self, name: &str, version: &str) -> Result<()> {
        let mut staged = tempfile::NamedTempFile::new_in(self.root.join(name))?;
        use std::io::Write;
        staged.write_all(version.as_bytes())?;
        staged.persist(self.root.join(name).join("active"))?;
        Ok(())
    }
    pub async fn call(
        &self,
        pin: &Pin,
        scope: &str,
        operation: Operation,
        timeout: Duration,
    ) -> Result<Value> {
        self.call_configured(pin, scope, operation, timeout, None)
            .await
    }
    pub async fn call_configured(
        &self,
        pin: &Pin,
        scope: &str,
        operation: Operation,
        timeout: Duration,
        environment: Option<&ProcessEnvironment>,
    ) -> Result<Value> {
        let configured_environment = self.configured_environment(pin, environment);
        let environment = configured_environment.as_ref().or(environment);
        let environment_digest = environment
            .map(|e| serde_json::to_vec(&e.values).map(|v| digest(&v)))
            .transpose()?
            .unwrap_or_default();
        let key = (
            pin.name.clone(),
            pin.version.clone(),
            scope.to_owned(),
            format!("{}:{environment_digest}", pin.sha256),
        );
        let client = {
            let mut clients = self.clients.lock().await;
            if clients.get(&key).is_some_and(|client| {
                client
                    .connection
                    .try_lock()
                    .is_ok_and(|state| state.is_none())
            }) {
                clients.remove(&key);
            }
            if let Some(client) = clients.get(&key) {
                client.clone()
            } else {
                let (installed, executable) = self.executable(pin)?;
                ensure!(
                    installed.manifest.credential.is_none(),
                    "provider requires a prepared credential"
                );
                let client = Arc::new(
                    Client::start_configured(
                        &executable,
                        &pin.name,
                        installed.package.protocol_version,
                        environment,
                    )
                    .await?,
                );
                ensure!(
                    client.manifest.provider_version == pin.version,
                    "provider version changed"
                );
                clients.insert(key, client.clone());
                client
            }
        };
        client.call(operation, timeout).await
    }
    fn configured_environment(
        &self,
        pin: &Pin,
        environment: Option<&ProcessEnvironment>,
    ) -> Option<ProcessEnvironment> {
        let Some(catalog) = self.catalog.as_ref() else {
            // A local installed AWS provider needs the operator's trusted runtime
            // paths, even when the broker supplies credentials in a private frame.
            // Do not persist these paths on connections or inherit AWS secrets.
            if pin.name == "aws" {
                let mut environment = environment?.clone();
                for key in ["MAGICMTN_AWS_PYTHON", "MAGICMTN_AWS_KUBECTL"] {
                    if let Ok(value) = std::env::var(key) {
                        environment.values.entry(key.into()).or_insert(value);
                    }
                }
                return Some(environment);
            }
            return None;
        };
        let mut values = catalog
            .iter()
            .find(|release| release.installed.pin() == *pin)
            .map(|release| release.runtime_environment.clone())
            .unwrap_or_default();
        if let Some(environment) = environment {
            values.extend(environment.values.clone());
        }
        Some(ProcessEnvironment { values })
    }

    fn executable(&self, pin: &Pin) -> Result<(Installed, PathBuf)> {
        let installed = self.installed_pin(pin)?;
        let executable = if let Some(catalog) = &self.catalog {
            catalog
                .iter()
                .find(|r| r.installed.pin() == *pin)
                .and_then(|r| r.executable.clone())
                .context("provider requires a connected runner")?
        } else {
            self.directory(&pin.name, &pin.version)?.join("provider")
        };
        ensure!(
            installed.package.sha256 == pin.sha256
                && digest(&std::fs::read(&executable)?) == pin.sha256,
            "installed provider checksum changed"
        );
        Ok((installed, executable.canonicalize()?))
    }

    pub async fn release(&self, scope: &str) {
        self.credentials.release(scope);
        let removed = {
            let mut clients = self.clients.lock().await;
            let keys: Vec<_> = clients
                .keys()
                .filter(|k| k.2 == scope || k.2.starts_with(&format!("{scope}:")))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|k| clients.remove(&k))
                .collect::<Vec<_>>()
        };
        for client in removed {
            client.shutdown().await;
        }
    }
    pub async fn shutdown(&self) {
        self.credentials.clear();
        let clients: Vec<_> = self
            .clients
            .lock()
            .await
            .drain()
            .map(|(_, client)| client)
            .collect();
        for client in clients {
            client.shutdown().await;
        }
    }
    /// Caller must hold `gate` and check persistent session/sandbox references first.
    pub async fn remove(&self, name: &str, version: &str) -> Result<()> {
        ensure!(
            !self.immutable(),
            "provider releases are immutable deployment content"
        );
        let directory = self.directory(name, version)?;
        self.installed(name, version)?;
        let clients = self.clients.lock().await;
        ensure!(
            !clients.keys().any(|k| k.0 == name && k.1 == version),
            "provider still has live RPC clients"
        );
        drop(clients);
        if self
            .active(name)
            .is_ok_and(|p| p.package.version == version)
        {
            std::fs::remove_file(self.root.join(name).join("active"))?;
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn nitro_startup_errors_are_allowlisted_without_forwarding_arbitrary_details() {
        let mut error = ProviderError {
            code: ErrorCode::UpstreamUnavailable,
            retryable: false,
            message: "Nitro sandbox image could not be pulled".into(),
        };
        assert_eq!(
            nitro_startup_error("aws", &error),
            Some("Nitro sandbox image could not be pulled")
        );
        assert_eq!(nitro_startup_error("other", &error), None);
        error.retryable = true;
        assert_eq!(nitro_startup_error("aws", &error), None);
        error.retryable = false;
        error.message.push_str(": secret upstream details");
        assert_eq!(nitro_startup_error("aws", &error), None);
        error.message = "Nitro exec failed; reconcile before retrying".into();
        assert_eq!(nitro_startup_error("aws", &error), None);
    }

    #[test]
    fn retained_chat_translates_history_without_changing_public_input() {
        let input = serde_json::json!({"messages":[
            {"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"run_command","arguments":"{\"program\":\"echo\",\"args\":[]}"}}]},
            {"role":"tool","tool_call_id":"call_1","name":"run_command","content":"ok"}
        ]});
        let mut operation = Operation::Infer {
            request: magicmtn_sdk::InferenceRequest {
                model: magicmtn_sdk::inference::ModelConfiguration {
                    model: "test".into(),
                    provider: None,
                    version: None,
                    parameters: serde_json::json!({}),
                },
                task: "chat".into(),
                input: input.clone(),
            },
        };
        retained_chat_input(&mut operation).unwrap();
        let Operation::Infer { request } = operation else {
            unreachable!()
        };
        assert_eq!(
            request.input["messages"][0]["tool_calls"][0]["function"]["arguments"]["program"],
            "echo"
        );
        assert_eq!(request.input["messages"][1]["tool_name"], "run_command");
        assert!(request.input["messages"][1].get("tool_call_id").is_none());
        assert!(input["messages"][0]["tool_calls"][0]["function"]["arguments"].is_string());
    }

    #[test]
    fn unchanged_retained_upload_can_be_reenrolled_but_not_freshly_installed() {
        let root = tempfile::tempdir().unwrap();
        let registry = Registry::new(root.path().to_owned()).unwrap();
        let executable = b"retained fixture executable";
        let package = Package {
            name: "retained".into(),
            version: "0.1.0".into(),
            protocol_version: 3,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            sha256: digest(executable),
        };
        let archive = |package: &Package| {
            let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut tar = tar::Builder::new(encoder);
            for (name, body) in [
                ("package.json", serde_json::to_vec(package).unwrap()),
                ("provider", executable.to_vec()),
                ("LICENSE", b"fixture".to_vec()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, &body[..]).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(registry.install(&archive(&package)).await.is_err());
            let directory = registry.directory(&package.name, &package.version).unwrap();
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("package.json"),
                serde_json::to_vec(&package).unwrap(),
            )
            .unwrap();
            std::fs::write(directory.join("provider"), executable).unwrap();
            let manifest = ProviderManifest {
                credential: None,
                manifest_version: MANIFEST_VERSION,
                provider_version: package.version.clone(),
                capabilities: magicmtn_sdk::plugin::Capabilities::inference(vec!["chat".into()]),
            };
            std::fs::write(
                directory.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(registry.install(&archive(&package)).await.unwrap().active);
            let mut mislabeled = package.clone();
            mislabeled.protocol_version = PROTOCOL_VERSION;
            assert!(registry.install(&archive(&mislabeled)).await.is_err());
        });
    }

    #[test]
    fn retained_protocol_is_explicit_and_public_clients_stay_v1() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("provider");
        let reply = serde_json::to_string(&Response {
            id: 1,
            protocol_version: 3,
            provider: "retained".into(),
            outcome: Outcome::Ok(Value::Description(ProviderManifest {
                credential: None,
                manifest_version: MANIFEST_VERSION,
                provider_version: "0.1.0".into(),
                capabilities: magicmtn_sdk::plugin::Capabilities::inference(vec![]),
            })),
        })
        .unwrap();
        std::fs::write(&path, format!(
            "#!/bin/sh\nread frame\ncase \"$frame\" in *'\"protocol_version\":3'*) printf '%s\\n' '{reply}';; *) exit 1;; esac\n"
        )).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                assert!(Client::start(&path, "retained").await.is_err());
                let client = Client::start_installed(&path, "retained", 3).await.unwrap();
                client.shutdown().await;
                assert!(Client::start_installed(&path, "retained", 2).await.is_err());
            });
    }
}
