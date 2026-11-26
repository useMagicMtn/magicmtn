use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde_json::{json, Value};
use std::{
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

mod agents;
mod authentication;
mod completions;
mod configuration;
mod execution;
mod images;
mod mcp;
mod mode;
mod opencode;
mod pi;
mod provider_registry;
mod sandbox_scp;
mod sandbox_shell;
mod telemetry;
mod uninstall;
mod update;

#[derive(Parser)]
#[command(
    name = "magic",
    about = concat!("MagicMtn CLI ", env!("CARGO_PKG_VERSION")),
    version
)]
struct Cli {
    /// Connect to the single-user server started by `magic server start`
    #[arg(long, global = true, conflicts_with = "server_url")]
    local: bool,
    /// Select a server explicitly; credentials remain scoped to that URL
    #[arg(long, global = true)]
    server_url: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}

impl Cli {
    fn target(&self) -> Result<mode::Selection> {
        let cloud = matches!(
            self.command,
            Some(Commands::Login { .. } | Commands::Logout)
        );
        anyhow::ensure!(
            !cloud || !self.local,
            "Cloud login and logout use the Cloud server; omit --local"
        );
        if self.local {
            let directory = if std::env::var_os("MAGICMTN_LOCAL_DIRECTORY").is_some()
                || std::env::var_os("MAGICMTN_CONFIG_DIRECTORY").is_some()
            {
                magicmtn_local::directory()?
            } else if let Some(mode::Selection::Local { directory }) = mode::selected()? {
                directory
            } else {
                magicmtn_local::directory()?
            };
            return Ok(mode::Selection::Local { directory });
        }
        if let Some(server) = &self.server_url {
            return Ok(mode::Selection::Cloud {
                server: server.clone(),
            });
        }
        if cloud {
            if cfg!(feature = "development-server") && mode::selected()?.is_none() {
                return Ok(mode::Selection::Cloud {
                    server: mode::default_server().to_owned(),
                });
            }
            return Ok(mode::Selection::Cloud {
                server: mode::cloud_server()?,
            });
        }
        if let Some(selection) = mode::selected()? {
            return Ok(selection);
        }
        Ok(mode::Selection::Cloud {
            server: std::env::var("MAGICMTN_SERVER_URL")
                .unwrap_or_else(|_| mode::default_server().to_owned()),
        })
    }

    fn server_url(&self) -> Result<String> {
        match self.target()? {
            mode::Selection::Cloud { server } => Ok(server),
            mode::Selection::Local { directory } => Ok(magicmtn_local::connection(&directory)?.url),
        }
    }
}

#[cfg(all(test, not(feature = "development-server")))]
#[test]
fn cloud_is_default_and_server_selection_is_explicit() {
    assert_eq!(mode::default_server(), "https://api.magicmtn.ai");
    let cli = Cli::try_parse_from(["magic", "--server-url", "http://127.0.0.1:1924"]).unwrap();
    assert_eq!(cli.server_url().unwrap(), "http://127.0.0.1:1924");
    assert!(
        Cli::try_parse_from(["magic", "--local", "--server-url", "http://127.0.0.1:1924"]).is_err()
    );
}

#[derive(Subcommand)]
enum Commands {
    /// Run the single-user MagicMtn server on this machine
    #[command(display_order = 8)]
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Manage sandbox images
    #[command(arg_required_else_help = true, display_order = 9)]
    Images {
        #[command(subcommand)]
        command: ImageCommands,
    },
    /// Expose MagicMtn sandbox tools to MCP clients
    #[command(display_order = 5)]
    Mcp {
        #[command(subcommand)]
        command: mcp::Commands,
    },
    /// Update magic to the latest public release
    #[command(display_order = 6)]
    Update,
    /// Sign in through the MagicMtn web app
    #[command(display_order = 0)]
    Login {
        /// Application initiating the browser sign-in
        #[arg(long, value_parser = ["cli", "pi"], default_value = "cli")]
        client: String,
    },
    /// Revoke the saved CLI session and sign out
    #[command(display_order = 1001)]
    Logout,
    /// Show your signed-in account
    #[command(display_order = 7)]
    Whoami,
    /// Manage Pi workspaces and inference
    #[command(display_order = 1)]
    Pi {
        #[command(subcommand)]
        command: pi::Commands,
    },
    /// Manage OpenCode workspaces
    #[command(name = "opencode")]
    OpenCode {
        #[command(subcommand)]
        command: opencode::Commands,
    },
    /// Install agents in your MagicMtn workspace
    #[command(display_order = 4)]
    Agents {
        #[command(subcommand)]
        command: agents::Commands,
    },
    /// Install and manage local providers
    #[command(display_order = 3)]
    Provider {
        #[command(subcommand)]
        command: ProviderCommands,
    },
    /// Manage standalone and session machines
    #[command(arg_required_else_help = true, display_order = 2)]
    Computer {
        #[command(subcommand)]
        command: ComputerCommands,
    },
    /// Generate and install shell completions (detect the shell if omitted)
    #[command(display_order = 1000)]
    Completion {
        #[arg(value_enum)]
        shell: Option<CompletionShell>,
        /// Print the completion script without changing shell configuration
        #[arg(long)]
        print: bool,
        /// Also add this absolute directory to your shell PATH
        #[arg(long, conflicts_with = "print")]
        path: Option<PathBuf>,
        /// Allow shell configuration changes without prompting
        #[arg(long, conflicts_with = "print")]
        yes: bool,
    },
    /// Uninstall this CLI and its shell and agent integrations after confirmation
    #[command(display_order = 1002)]
    Uninstall,
}

#[derive(Subcommand)]
enum ImageCommands {
    /// List images available in the selected deployment
    List {
        #[arg(long, value_enum, default_value = "human")]
        format: ShowFormat,
    },
    /// Register a custom OCI image in the local deployment
    Add {
        name: String,
        #[arg(long)]
        from: String,
    },
    /// Prepare a registered image on a connected sandbox provider
    Prepare {
        name: String,
        #[arg(long)]
        provider: String,
    },
}

#[derive(Subcommand)]
enum ServerCommands {
    /// Stop the local server after active requests finish
    Stop,
    /// Initialize local state and serve sandbox operations until interrupted
    Start {
        #[arg(long, default_value = "127.0.0.1:1924")]
        bind: std::net::SocketAddr,
        /// Loopback address for the web dashboard
        #[arg(long, default_value = "127.0.0.1:1900")]
        ui_bind: std::net::SocketAddr,
        #[arg(long, default_value_t = 86400, value_parser = clap::value_parser!(u32).range(1..=86400))]
        ttl_seconds: u32,
        /// Override the Apple Container network for new sandboxes; use none for isolation
        #[arg(long, value_parser = magicmtn_local::network_name)]
        network: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProviderCommands {
    /// Remove all locally installed versions of a provider
    Uninstall { name: String },
    /// Install a provider from the MagicMtn Registry or a local package
    Install {
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        name: Option<String>,
        /// Install a local provider package instead of downloading from the Registry
        #[arg(long, value_name = "PACKAGE")]
        file: Option<PathBuf>,
    },
    /// Configure an installed provider account
    Configure {
        name: String,
        #[arg(
            long,
            required_unless_present = "secret_file",
            conflicts_with = "secret_file",
            value_name = "JSON"
        )]
        secret_string: Option<String>,
        #[arg(long, value_name = "PATH")]
        secret_file: Option<PathBuf>,
        /// Provider setup option, such as app=magicmtn; may be repeated
        #[arg(long = "option", value_name = "KEY=VALUE")]
        options: Vec<String>,
    },
    /// Check the configured account without creating provider resources
    Verify { name: String },
    /// List locally installed providers, or full metadata as JSON
    List {
        #[arg(long, value_enum, default_value = "human")]
        format: ShowFormat,
    },
}

#[derive(Subcommand)]
enum ComputerCommands {
    /// Create a managed computer, or use an explicit provider image
    New {
        #[arg(long)]
        image: Option<String>,
        /// Select a MagicMtn inference model (defaults to your saved Pi preference)
        #[arg(long, conflicts_with_all = ["image", "pi_connection"])]
        model: Option<uuid::Uuid>,
        /// Use an explicitly connected Pi inference profile
        #[arg(long, conflicts_with = "image")]
        pi_connection: Option<uuid::Uuid>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, requires = "image")]
        connection: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Execute a bounded command in a computer
    Exec {
        id: String,
        #[arg(long, default_value_t = 30000, value_parser = clap::value_parser!(u64).range(1..=30000))]
        timeout_ms: u64,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Open an interactive shell; exit returns to your terminal
    Ssh { id: String },
    /// Copy a local file into the computer's default working directory
    Scp { id: String, file: PathBuf },
    /// List active computers, including those created by agent sessions
    List {
        /// Include all computers regardless of status
        #[arg(long)]
        all: bool,
    },
    /// Show a computer by full UUID or unique prefix
    Show {
        id: String,
        #[arg(long, value_enum, default_value = "human")]
        format: ShowFormat,
    },
    /// Terminate a computer and end any attached sessions
    Terminate {
        id: String,
        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ShowFormat {
    Human,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}
impl From<CompletionShell> for Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::Bash => Self::Bash,
            CompletionShell::Zsh => Self::Zsh,
            CompletionShell::Fish => Self::Fish,
        }
    }
}

fn append_line_if_missing(path: &Path, line: &str) -> Result<()> {
    if path.exists()
        && fs::read_to_string(path)?
            .lines()
            .any(|existing| existing == line)
    {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    writeln!(file, "\n{line}")?;
    Ok(())
}

fn prepend_line_if_missing(path: &Path, line: &str) -> Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if existing.lines().any(|candidate| candidate == line) {
        return Ok(());
    }
    fs::write(path, format!("{line}\n{existing}"))?;
    Ok(())
}

fn shell_named(name: &str) -> Option<Shell> {
    let name = Path::new(name).file_name()?.to_str()?;
    match name.trim_start_matches('-') {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        _ => None,
    }
}

fn process_field(pid: u32, field: &str) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", field, "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn parent_shell() -> Option<Shell> {
    // Follow the installer shell (for example, `curl | sh`) to the shell that
    // launched it. SHELL is the fallback when no supported parent is found.
    let mut pid = std::process::id();
    for _ in 0..2 {
        pid = process_field(pid, "ppid=")?.parse().ok()?;
        if pid <= 1 {
            break;
        }
        if let Some(shell) = shell_named(&process_field(pid, "comm=")?) {
            return Some(shell);
        }
    }
    None
}

fn current_shell() -> Option<Shell> {
    parent_shell().or_else(|| {
        let shell = std::env::var_os("SHELL")?;
        shell_named(shell.to_str()?)
    })
}

fn completion_auto(path: Option<&Path>, yes: bool) -> Result<()> {
    match current_shell() {
        Some(shell) => completion(shell, false, path, yes),
        None => {
            eprintln!("Shell completions skipped: set SHELL to bash, zsh, or fish, or run `magic completion <shell>`.");
            Ok(())
        }
    }
}

fn completion(shell: Shell, print: bool, install_dir: Option<&Path>, yes: bool) -> Result<()> {
    let mut command = Cli::command();
    let script = completions::generate(shell, &mut command);
    if print {
        std::io::stdout().write_all(&script)?;
        return Ok(());
    }
    let home = PathBuf::from(std::env::var("HOME").context("HOME environment variable not set")?);
    let (directory, filename) = match shell {
        Shell::Bash => (home.join(".local/share/magic/completions"), "magic.bash"),
        Shell::Zsh => (home.join(".local/share/magic/completions"), "_magic"),
        Shell::Fish => (completions::fish_directory(&home), "magic.fish"),
        _ => bail!("unsupported shell"),
    };
    let path = directory.join(filename);
    let quote = |path: &Path| path.to_string_lossy().replace('\'', "'\\''");
    let rc = match shell {
        Shell::Bash if cfg!(target_os = "macos") => [".bash_profile", ".bash_login", ".profile"]
            .iter()
            .map(|name| home.join(name))
            .find(|path| path.exists())
            .unwrap_or_else(|| home.join(".bash_profile")),
        Shell::Bash => home.join(".bashrc"),
        Shell::Zsh => std::env::var_os("ZDOTDIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.clone())
            .join(".zshrc"),
        Shell::Fish => directory
            .parent()
            .context("missing fish directory")?
            .join("conf.d/magic-path.fish"),
        _ => bail!("unsupported shell"),
    };
    let mut lines = Vec::new();
    if let Some(install_dir) = install_dir {
        anyhow::ensure!(
            install_dir.is_absolute(),
            "--path must be an absolute directory"
        );
        let quoted = quote(install_dir);
        let line = if shell == Shell::Fish {
            // Fish single quotes escape backslashes as well as apostrophes.
            let quoted = install_dir
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('\'', "\\'");
            format!("fish_add_path -- '{quoted}'")
        } else {
            format!("export PATH='{quoted}':\"$PATH\"")
        };
        println!("To use magic in this shell now, run: {line}");
        lines.push(line);
    }
    match shell {
        Shell::Bash => lines.push(format!("source '{}'", quote(&path))),
        Shell::Zsh => lines.push(format!("fpath=('{}' $fpath)", quote(&directory))),
        _ => (),
    }
    let existing = match fs::read_to_string(&rc) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    lines.retain(|line| !existing.lines().any(|existing| existing == line));
    // Fish loads its completion file automatically, so creating it is also a
    // shell configuration change even when there is no rc line to add.
    let needs_consent = !lines.is_empty() || (shell == Shell::Fish && !path.exists());
    if needs_consent && !yes {
        let prompt = format!(
            "Set up magic {} in {} and {}? [Y/n] ",
            if install_dir.is_some() {
                "PATH and completions"
            } else {
                "completions"
            },
            rc.display(),
            directory.display()
        );
        if !confirm_shell_setup(&prompt)? {
            println!("Shell setup skipped. Run magic completion {shell} --yes to install completions later.");
            return Ok(());
        }
    }
    fs::create_dir_all(&directory)?;
    fs::write(&path, script)?;
    if !lines.is_empty() {
        fs::create_dir_all(
            rc.parent()
                .context("missing shell configuration directory")?,
        )?;
        // Zsh's fpath must be set before an existing compinit invocation.
        if shell == Shell::Zsh {
            prepend_line_if_missing(&rc, &lines.join("\n"))?;
        } else {
            append_line_if_missing(&rc, &lines.join("\n"))?;
        }
    }
    println!(
        "Installed {shell} completions for magic. Restart your shell to load the configuration."
    );
    if shell == Shell::Fish {
        println!(
            "Reload completions in your current shell: magic completion fish --print | source"
        );
    }
    Ok(())
}

fn confirm_shell_setup(prompt: &str) -> Result<bool> {
    // curl | sh consumes stdin. Ask on the controlling terminal instead, and
    // leave configuration untouched when there is no interactive terminal.
    let Ok(mut terminal) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    else {
        return Ok(false);
    };
    write!(terminal, "{prompt}")?;
    terminal.flush()?;
    let mut answer = String::new();
    use std::io::BufRead;
    if std::io::BufReader::new(&terminal).read_line(&mut answer)? == 0 {
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

struct Api {
    client: reqwest::Client,
    base: reqwest::Url,
    token: Option<String>,
    local: bool,
}
fn http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(600));
    let certificate_path = std::env::var_os("MAGICMTN_CA_CERT").or_else(|| {
        let setting =
            std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config/magicmtn/ca-cert");
        std::fs::read_to_string(setting)
            .ok()
            .map(|s| std::ffi::OsString::from(s.trim()))
    });
    if let Some(path) = certificate_path {
        let bytes = std::fs::read(path).context("reading MAGICMTN_CA_CERT")?;
        for certificate in reqwest::Certificate::from_pem_bundle(&bytes)? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder.build()?)
}
impl Api {
    fn new(base: &str) -> Result<Self> {
        let base = reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/')))?;
        anyhow::ensure!(
            matches!(base.scheme(), "http" | "https")
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "invalid server URL"
        );
        Ok(Self {
            client: http_client()?,
            base,
            token: None,
            local: false,
        })
    }
    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self.authenticated(self.client.request(method, self.base.join(path)?));
        if self.local && path.starts_with("v1/images/") && path.ends_with("/prepare") {
            request = request.timeout(std::time::Duration::from_secs(1800));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.context(if self.local {
            "could not reach the local MagicMtn server; run `magic server start` and retry"
        } else {
            "could not reach MagicMtn server"
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            if self.local {
                bail!("local server rejected its private credential; restart the local server and retry");
            }
            bail!("not signed in or session expired; run `magic login` for this server");
        }
        if !status.is_success() {
            bail!("server returned {status}: {}", response.text().await?);
        }
        Ok(response.json().await?)
    }
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn validate_id(id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 36 && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
        "expected a sandbox UUID or hexadecimal prefix"
    );
    Ok(())
}
fn table_cell(value: &str, width: usize) -> String {
    let mut chars = value.chars().map(|c| if c.is_control() { ' ' } else { c });
    let mut cell: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() {
        cell = cell.chars().take(width - 3).collect();
        cell.push_str("...");
    }
    cell
}
fn sandbox_image(record: &Value) -> &str {
    text(record, "template").trim_prefix("image:")
}
fn print_list(items: &[Value]) {
    println!(
        "{:<36} | {:<12} | {:<20} | {:<16} | created",
        "id", "state", "notes", "image"
    );
    for item in items {
        println!(
            "{:<36} | {:<12} | {:<20} | {:<16} | {}",
            text(item, "id"),
            text(item, "status"),
            table_cell(text(item, "notes"), 20),
            table_cell(sandbox_image(item), 16),
            text(item, "created_at")
                .get(..19)
                .unwrap_or_else(|| text(item, "created_at"))
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let operation = match &cli.command {
        Some(Commands::Pi { command }) => Some(command.operation()),
        _ => None,
    };
    let server_url = cli.server_url().unwrap_or_default();
    let operation = operation.filter(|_| {
        server_url == "https://api.magicmtn.ai" && configuration::error_reporting().unwrap_or(false)
    });
    let sentry = operation.map(|_| telemetry::init());
    let result = run(cli).await;
    if let (Some(operation), Err(error)) = (operation, &result) {
        telemetry::report("magic-cli", operation, error);
    }
    drop(sentry);
    result
}

async fn run(mut cli: Cli) -> Result<()> {
    let Some(command) = cli.command.take() else {
        Cli::command().print_help()?;
        return Ok(());
    };
    if let Commands::Server { command } = command {
        return match command {
            ServerCommands::Start {
                bind,
                ui_bind,
                ttl_seconds,
                network,
            } => {
                let transition = mode::transition()?;
                authentication::Credentials::new()?
                    .ensure_signed_out(&reqwest::Url::parse(&mode::cloud_server()?)?)?;
                let directory = std::path::absolute(magicmtn_local::directory()?)?;
                let mut running = mode::server()?;
                drop(transition);
                magicmtn_local::run_with_ready(
                    magicmtn_local::Options {
                        bind,
                        ui_bind,
                        ttl_seconds,
                        network,
                        directory: directory.clone(),
                        provider_directory: configuration::directory()?.join("providers"),
                    },
                    || {
                        mode::ready(&mut running, &directory)?;
                        mode::select(&mode::Selection::Local { directory })
                    },
                )
                .await
            }
            ServerCommands::Stop => {
                let _transition = mode::transition()?;
                let directory = match mode::running_directory()? {
                    Some(directory) => directory,
                    None => magicmtn_local::directory()?,
                };
                anyhow::ensure!(
                    magicmtn_local::is_running(&directory)?,
                    "local MagicMtn server is stopped"
                );
                let info = magicmtn_local::connection(&directory)?;
                let mut api = Api::new(&info.url)?;
                anyhow::ensure!(
                    api.base.scheme() == "http"
                        && matches!(
                            api.base.host_str(),
                            Some("127.0.0.1" | "localhost" | "[::1]")
                        ),
                    "local connection must use loopback HTTP"
                );
                api.token = Some(info.token);
                api.request(reqwest::Method::POST, "shutdown", None).await?;
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    while magicmtn_local::is_running(&directory)?
                        || mode::running_directory()?.as_ref() == Some(&directory)
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    anyhow::Ok(())
                })
                .await
                .context("waiting for local MagicMtn server to stop")??;
                println!("MagicMtn local server stopped.");
                Ok(())
            }
        };
    }
    if let Commands::Uninstall = command {
        return uninstall::run(&cli.server_url()?).await;
    }
    if let Commands::Completion {
        shell,
        print,
        path,
        yes,
    } = command
    {
        return match shell {
            Some(shell) => completion(shell.into(), print, path.as_deref(), yes),
            None if print => bail!("specify a shell when using --print"),
            None => completion_auto(path.as_deref(), yes),
        };
    }
    if let Commands::Update = command {
        update::run().await?;
        return Ok(());
    }
    if let Commands::Images { command } = &command {
        match cli.target()? {
            mode::Selection::Local { directory } => {
                match command {
                    ImageCommands::List { format } => return images::list_local(&directory, *format).await,
                    ImageCommands::Add { name, from } => return images::add(&directory, name, from).await,
                    ImageCommands::Prepare { .. } => {}
                }
            }
            mode::Selection::Cloud { .. } => anyhow::ensure!(matches!(command, ImageCommands::List { .. }),
                "start the standalone server with `magic server start` before registering or preparing images"),
        }
    }
    if let Commands::Provider { command } = &command {
        match command {
            ProviderCommands::Install { name, file } => {
                return match (name.as_deref(), file.as_deref()) {
                    (Some(name), None) => provider_registry::install(name).await,
                    (None, Some(file)) => provider_registry::install_file(file).await,
                    _ => unreachable!("provider install requires exactly one package source"),
                }
            }
            ProviderCommands::Uninstall { name } => {
                return provider_registry::uninstall(name).await
            }
            ProviderCommands::List { format } => return provider_registry::list(*format),
            ProviderCommands::Configure { .. } | ProviderCommands::Verify { .. } => {}
        }
    }
    cli.command = Some(command);
    let target = cli.target()?;
    let command = cli.command.take().unwrap();
    let (server_url, local_directory) = match target {
        mode::Selection::Cloud { server } => (server, None),
        mode::Selection::Local { directory } => {
            (magicmtn_local::connection(&directory)?.url, Some(directory))
        }
    };
    let local = local_directory.is_some();
    if matches!(
        &command,
        Commands::Provider {
            command: ProviderCommands::Configure { .. }
        }
    ) {
        anyhow::ensure!(
            local,
            "start the standalone server with `magic server start` before configuring a provider"
        );
    }
    let mut api = Api::new(&server_url)?;
    api.local = local;
    if let Some(directory) = local_directory {
        let info = magicmtn_local::connection(&directory)?;
        let url = reqwest::Url::parse(&info.url)?;
        anyhow::ensure!(
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
            "local connection must use loopback HTTP"
        );
        anyhow::ensure!(
            Api::new(&info.url)?.base == api.base,
            "local server connection changed; retry the command"
        );
        api.token = Some(info.token);
    } else if !matches!(command, Commands::Login { .. }) {
        api.token = authentication::Credentials::new()?.load(&api.base)?;
    }
    match command {
        Commands::Images {
            command: ImageCommands::List { format },
        } => return images::list_cloud(&api, format).await,
        Commands::Images {
            command: ImageCommands::Prepare { name, provider },
        } => return images::prepare(&api, &name, &provider).await,
        Commands::Provider {
            command: ProviderCommands::Verify { name },
        } => {
            anyhow::ensure!(
                local,
                "start the standalone server with `magic server start` before verifying a provider"
            );
            anyhow::ensure!(
                provider_registry::valid_name(&name),
                "invalid provider name"
            );
            api.request(Method::POST, &format!("v1/providers/{name}/verify"), None)
                .await?;
            println!("Provider {name} verified.");
            return Ok(());
        }
        Commands::Provider {
            command:
                ProviderCommands::Configure {
                    name,
                    secret_string,
                    secret_file,
                    options,
                },
        } => {
            return provider_registry::configure(
                &api,
                &name,
                secret_string,
                secret_file.as_deref(),
                &options,
            )
            .await;
        }
        Commands::Login { client } => return authentication::login(&api, &client).await,
        Commands::Logout => return authentication::logout(&api).await,
        Commands::Whoami => {
            if local {
                println!("Mode: Local\nServer: {}", api.base);
            } else {
                println!("{}", authentication::whoami(&api).await?);
            }
            return Ok(());
        }
        Commands::Pi { command } => return pi::run(&api, command).await,
        Commands::OpenCode { command } => return opencode::run(&api, command).await,
        Commands::Mcp { command } => return mcp::run(&api, command).await,
        Commands::Agents { command } => return agents::run(&api, command).await,
        _ => (),
    }
    let Commands::Computer { command } = command else {
        unreachable!()
    };
    use reqwest::Method;
    match command {
        ComputerCommands::New {
            image,
            model,
            pi_connection,
            provider,
            connection,
            notes,
        } => {
            anyhow::ensure!(
                !local || (model.is_none() && pi_connection.is_none()),
                "local sandboxes use the agent's own inference; omit --model and --pi-connection"
            );
            anyhow::ensure!(
                local || image.is_some() || (provider.is_none() && notes.is_none()),
                "--provider and --notes require --image on the Cloud server"
            );
            let key = uuid::Uuid::new_v4();
            let (route, body) = if local {
                let mut body = images::creation_body(image.as_deref(), provider.as_deref(), key)?;
                body["connection_id"] = json!(connection);
                body["notes"] = json!(notes);
                ("v1/sandboxes", body)
            } else {
                match image {
                    Some(image) => (
                        "v1/sandboxes",
                        json!({
                            "template": format!("image:{image}"),
                            "provider": provider.unwrap_or_else(|| "apple-container".into()),
                            "connection_id": connection,
                            "idempotency_key": key,
                            "notes": notes,
                        }),
                    ),
                    None => (
                        "v1/sandboxes/managed",
                        pi::managed_creation_body(&api, model, pi_connection, key).await?,
                    ),
                }
            };
            let result = api.request(Method::POST, route, Some(body)).await?;
            let id = text(&result, "id");
            uuid::Uuid::parse_str(id)
                .context("sandbox creation response did not contain a valid sandbox ID")?;
            if std::io::stdout().is_terminal() {
                eprintln!("Sandbox {id} created successfully");
            }
            println!("{id}");
        }
        ComputerCommands::Exec {
            id,
            timeout_ms,
            command,
        } => {
            validate_id(&id)?;
            let result = api.request(Method::POST, &format!("v1/sandboxes/{id}/exec"), Some(json!({
                "command": command[0], "args": command[1..], "timeout_ms": timeout_ms, "max_output_bytes": 16384,
            }))).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            anyhow::ensure!(
                result["exit_code"] == 0 && result["timed_out"] != true,
                "sandbox command failed or timed out"
            );
        }
        ComputerCommands::Ssh { id } => {
            validate_id(&id)?;
            if local {
                sandbox_shell::run(&api, &id).await?;
            } else {
                pi::sandbox_shell(&api, &id).await?;
            }
        }
        ComputerCommands::Scp { id, file } => sandbox_scp::run(&api, &id, &file).await?,
        ComputerCommands::List { all } => {
            let result = api.request(Method::GET, "v1/sandboxes", None).await?;
            let items: Vec<_> = result
                .as_array()
                .context("invalid sandbox list")?
                .iter()
                .filter(|s| all || s["status"] == "running")
                .cloned()
                .collect();
            print_list(&items);
        }
        ComputerCommands::Show { id, format } => {
            validate_id(&id)?;
            let result = api
                .request(Method::GET, &format!("v1/sandboxes/{id}"), None)
                .await?;
            match format {
                ShowFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                ShowFormat::Human => {
                    for key in [
                        "id",
                        "provider_id",
                        "status",
                        "image",
                        "notes",
                        "created_at",
                    ] {
                        let value = if key == "image" {
                            sandbox_image(&result)
                        } else {
                            text(&result, key)
                        };
                        println!("{key:<12} {value}");
                    }
                    for key in [
                        "runtime_state",
                        "runtime_checked_at",
                        "runtime_error",
                        "shutdown_reason",
                    ] {
                        if !result[key].is_null() {
                            println!("{key:<18} {}", text(&result, key));
                        }
                    }
                }
            }
        }
        ComputerCommands::Terminate { id, yes } => {
            validate_id(&id)?;
            if !yes {
                anyhow::ensure!(
                    std::io::stdin().is_terminal(),
                    "sandbox termination requires confirmation; use --yes in scripts"
                );
                if !confirm_sandbox_termination(
                    &mut std::io::stdin().lock(),
                    &mut std::io::stderr().lock(),
                )? {
                    eprintln!("Termination cancelled.");
                    return Ok(());
                }
            }
            let result = api
                .request(
                    Method::POST,
                    &format!("v1/sandboxes/{id}/terminate"),
                    Some(json!({})),
                )
                .await?;
            if std::io::stdout().is_terminal() {
                println!("{}", sandbox_termination_message(&result));
            } else {
                println!("{}", text(&result, "id"));
            }
        }
    }
    Ok(())
}

fn sandbox_termination_message(result: &Value) -> String {
    let action = if result["status"] == "ended" {
        "Terminated"
    } else {
        "Termination queued for"
    };
    let id = text(result, "id");
    match text(result, "provider_id") {
        "" => format!("{action} sandbox {id}"),
        provider_id => format!("{action} sandbox {id} ({provider_id})"),
    }
}

fn confirm_sandbox_termination(
    input: &mut impl std::io::BufRead,
    output: &mut impl Write,
) -> Result<bool> {
    write!(
        output,
        "Are you sure you want to terminate this sandbox? [y/N] "
    )?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
#[test]
fn provider_configuration_requires_one_json_source() {
    let base = ["magic", "provider", "configure", "modal"];
    for arguments in [
        vec!["--secret-string", "{}"],
        vec!["--secret-file", "secret.json"],
    ] {
        assert!(Cli::try_parse_from(base.into_iter().chain(arguments)).is_ok());
    }
    assert!(Cli::try_parse_from(base).is_err());
    assert!(Cli::try_parse_from(base.into_iter().chain([
        "--secret-string",
        "{}",
        "--secret-file",
        "secret.json",
    ]))
    .is_err());
}

#[cfg(test)]
#[test]
fn provider_install_accepts_one_source_and_rejects_the_retired_command() {
    let cli = Cli::try_parse_from(["magic", "provider", "install", "apple-container"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Provider {
            command: ProviderCommands::Install { name: Some(name), file: None }
        }) if name == "apple-container"
    ));
    let cli =
        Cli::try_parse_from(["magic", "provider", "install", "--file", "provider.tar.gz"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Provider {
            command: ProviderCommands::Install { name: None, file: Some(file) }
        }) if file == Path::new("provider.tar.gz")
    ));
    for args in [
        vec!["magic", "provider", "install"],
        vec!["magic", "provider", "install", "--file"],
        vec![
            "magic",
            "provider",
            "install",
            "apple-container",
            "--file",
            "provider.tar.gz",
        ],
        vec![
            "magic",
            "computer",
            "local-install",
            "--file",
            "provider.tar.gz",
        ],
        vec![
            "magic",
            "computer",
            "local-run",
            "--catalog",
            "catalog.json",
        ],
        vec![
            "magic",
            "computer",
            "local",
            "--connection",
            "11111111-1111-4111-8111-111111111111",
        ],
        vec!["magic", "provider", "add", "--file", "provider.tar.gz"],
        vec![
            "magic",
            "provider",
            "remove",
            "example",
            "--version",
            "1.0.0",
        ],
        vec!["magic", "internal-provider-runner"],
        vec!["magic", "computer", "create", "--image", "alpine"],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[cfg(test)]
#[test]
fn sandbox_termination_message_omits_empty_provider_id() {
    let id = "11111111-1111-4111-8111-111111111111";
    for (status, expected) in [
        ("ended", format!("Terminated sandbox {id}")),
        (
            "terminating",
            format!("Termination queued for sandbox {id}"),
        ),
    ] {
        let mut result = json!({"id": id, "status": status});
        assert_eq!(sandbox_termination_message(&result), expected);

        result["provider_id"] = json!("");
        assert_eq!(sandbox_termination_message(&result), expected);

        result["provider_id"] = json!("provider-sandbox-1");
        assert_eq!(
            sandbox_termination_message(&result),
            format!("{expected} (provider-sandbox-1)")
        );
    }
}

#[cfg(test)]
#[test]
fn sandbox_termination_requires_an_explicit_yes() {
    for (answer, expected) in [
        ("y\n", true),
        (" YES \n", true),
        ("\n", false),
        ("", false),
        ("no\n", false),
        ("anything else\n", false),
    ] {
        let mut output = Vec::new();
        assert_eq!(
            confirm_sandbox_termination(&mut answer.as_bytes(), &mut output).unwrap(),
            expected
        );
        assert_eq!(
            output,
            b"Are you sure you want to terminate this sandbox? [y/N] "
        );
    }
}

#[cfg(test)]
#[test]
fn pi_attach_rejects_removed_relay_options() {
    let id = "11111111-1111-4111-8111-111111111111";
    assert!(Cli::try_parse_from(["magic", "pi", "attach", id]).is_ok());
    assert!(Cli::try_parse_from(["magic", "pi", "attach", id, "--relay-from", "/repo"]).is_err());
    assert!(Cli::try_parse_from(["magic", "pi", "relay", id, "/repo"]).is_err());
}

#[cfg(test)]
#[test]
fn local_server_accepts_explicit_network_names() {
    let cli = Cli::try_parse_from(["magic", "server", "start"]).unwrap();
    let Some(Commands::Server {
        command: ServerCommands::Start { network, .. },
    }) = cli.command
    else {
        panic!("expected server start");
    };
    assert_eq!(network, None);

    for name in ["default", "none", "my-network.1", "local_net"] {
        let cli = Cli::try_parse_from(["magic", "server", "start", "--network", name]).unwrap();
        let Some(Commands::Server {
            command: ServerCommands::Start { bind, network, .. },
        }) = cli.command
        else {
            panic!("expected server start");
        };
        assert_eq!(bind, "127.0.0.1:1924".parse().unwrap());
        assert_eq!(network.as_deref(), Some(name));
    }
    for name in ["", "bad network", "a/b", ";echo", &"x".repeat(129)] {
        assert!(Cli::try_parse_from(["magic", "server", "start", "--network", name]).is_err());
    }
}

#[cfg(test)]
#[tokio::test]
async fn local_requests_never_request_cloud_login() {
    let app = axum::Router::new().route(
        "/private",
        axum::routing::get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut api = Api::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    api.local = true;
    api.token = Some("local-credential".into());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let error = api
        .request(reqwest::Method::GET, "private", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("private credential"), "{error}");
    assert!(!error.contains("magic login"), "{error}");
    server.abort();
}
