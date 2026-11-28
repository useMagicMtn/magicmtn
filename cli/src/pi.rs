mod streaming;
use crate::Api;
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    fs,
    io::{IsTerminal, Read, Write},
    mem::DropGuard,
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum Commands {
    /// Print the current sign-in state and public account profile as JSON
    #[command(hide = true)]
    Account,
    /// Install Pi through the agent registry
    #[command(hide = true)]
    Install,
    /// Print available models and the default image as JSON
    #[command(hide = true)]
    Catalog,
    /// Save the default inference model for this account and organization
    #[command(hide = true)]
    Defaults {
        #[arg(long, required_unless_present = "use_pi", conflicts_with = "use_pi")]
        model: Option<Uuid>,
        #[arg(long)]
        use_pi: bool,
    },
    /// List Pi workspaces as JSON
    List,
    /// Create a workspace using MagicMtn Canonical
    Create {
        /// MagicMtn model registration for workspace inference
        #[arg(
            long,
            required_unless_present = "pi_connection",
            conflicts_with = "pi_connection"
        )]
        model: Option<Uuid>,
        /// Explicitly connected Pi inference profile (credentials stay in the gateway)
        #[arg(long)]
        pi_connection: Option<Uuid>,
        /// UUID for this creation request (generated automatically if omitted).
        /// Reuse it only to inspect/recover the same creation request.
        #[arg(long, value_name = "UUID")]
        idempotency_key: Option<Uuid>,
    },
    /// Create a managed local Pi session using the prepared default image
    #[command(hide = true)]
    LocalCreate {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: Option<Uuid>,
        #[arg(long)]
        idempotency_key: Option<Uuid>,
    },
    /// Connect an API-key Pi model to the trusted gateway; read JSON on stdin
    #[command(hide = true)]
    Connect,
    /// List this account's connected inference profiles (never credentials)
    Connections,
    /// Revoke gateway access to a connected inference credential
    Disconnect { id: Uuid },
    /// Read durable agent-session status
    Status { id: Uuid },
    /// Stop the agent without deleting the sandbox
    Stop { id: Uuid },
    /// Explicitly restart the agent using its saved conversation
    Restart { id: Uuid },
    /// Connect your terminal to Pi inside a workspace; Ctrl+] disconnects
    Attach { id: Uuid },
    /// Terminate a workspace and delete its sandbox using durable sandbox cleanup
    Terminate { id: Uuid },
    /// Issue a short-lived, model-scoped provider configuration for the pi extension
    #[command(hide = true)]
    Inference {
        #[arg(long)]
        model: Uuid,
    },
    /// Report a Pi extension operation failure without forwarding its message
    #[command(hide = true)]
    ReportError {
        #[arg(long, value_parser = ["attach", "create", "resume", "restart"])]
        operation: String,
    },
}
impl Commands {
    pub(crate) fn operation(&self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Install => "install",
            Self::Catalog => "catalog",
            Self::Defaults { .. } => "defaults",
            Self::List => "list",
            Self::Create { .. } => "create",
            Self::LocalCreate { .. } => "create",
            Self::Connect => "connect",
            Self::Connections => "connections",
            Self::Disconnect { .. } => "disconnect",
            Self::Status { .. } => "status",
            Self::Stop { .. } => "stop",
            Self::Restart { .. } => "restart",
            Self::Attach { .. } => "attach",
            Self::Terminate { .. } => "terminate",
            Self::Inference { .. } => "inference",
            Self::ReportError { .. } => "report",
        }
    }
}
pub(crate) fn install_extension() -> Result<PathBuf> {
    let root = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or(
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".pi/agent"),
        );
    let directory = root.join("extensions/magicmtn");
    write_extension(&directory, &std::env::current_exe()?)?;
    Ok(directory)
}

fn write_extension(directory: &std::path::Path, executable: &std::path::Path) -> Result<()> {
    fs::create_dir_all(directory)?;
    fs::write(
        directory.join("index.js"),
        include_str!("../../interfaces/pi/index.js").replace(
            "const magicExecutable = null;",
            &format!(
                "const magicExecutable = {};",
                serde_json::to_string(executable)?
            ),
        ),
    )?;
    fs::write(
        directory.join("workspace.js"),
        include_str!("../../interfaces/pi/workspace.js"),
    )?;
    fs::write(
        directory.join("picker.js"),
        include_str!("../../interfaces/pi/picker.js"),
    )?;
    fs::write(
        directory.join("inference.js"),
        include_str!("../../interfaces/pi/inference.js"),
    )?;
    fs::write(
        directory.join("local.js"),
        include_str!("../../interfaces/pi/local.js"),
    )?;
    fs::write(directory.join("package.json"), "{\"type\":\"module\"}\n")?;
    Ok(())
}
pub async fn run(api: &Api, command: Commands) -> Result<()> {
    if api.local && !matches!(&command, Commands::Account | Commands::Install) {
        return run_local(api, command).await;
    }
    let result = match command {
        Commands::Account => crate::authentication::account(api).await?,
        Commands::Install => {
            return crate::agents::run(
                api,
                crate::agents::Commands::Install { agent: "pi".into() },
            )
            .await;
        }
        Commands::Catalog => catalog(api).await?,
        Commands::Defaults { model, use_pi } => {
            let catalog = catalog(api).await?;
            let selected_model = use_pi
                || catalog["models"]
                    .as_array()
                    .context("model catalog missing")?
                    .iter()
                    .any(|m| {
                        Some(m["id"].as_str().unwrap_or_default())
                            == model.as_ref().map(|id| id.to_string()).as_deref()
                    });
            ensure!(selected_model, "select an available MagicMtn model");
            let path = defaults_path(api, &catalog)?;
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent().context("preferences directory missing")?,
            )?;
            let mut defaults =
                json!({"model_id":model,"inference_source":if use_pi{"pi"}else{"magicmtn"}});
            if let Some(connection) = catalog["defaults"].get("pi_connection_id") {
                defaults["pi_connection_id"] = connection.clone();
            }
            serde_json::to_writer(&mut file, &defaults)?;
            file.flush()?;
            file.as_file().sync_all()?;
            file.persist(path).context("saving workspace defaults")?;
            json!({"saved":true})
        }
        Commands::List => api.request(Method::GET, "v1/pi/workspaces", None).await?,
        Commands::Create {
            model,
            pi_connection,
            idempotency_key,
        } => {
            let key = idempotency_key.unwrap_or_else(Uuid::new_v4);
            eprintln!("Creating workspace {key}. If interrupted, inspect `magic pi list`; do not create a replacement blindly.");
            api.request(
                Method::POST,
                "v1/pi/workspaces",
                Some(crate::configuration::creation_body(json!({"model_id":model,"pi_connection_id":pi_connection,"idempotency_key":key}))?),
            )
            .await?
        }
        Commands::LocalCreate { .. } => bail!("local-create requires local mode"),
        Commands::Connect => {
            let mut input = String::new();
            std::io::stdin().take(32769).read_to_string(&mut input)?;
            ensure!(input.len() <= 32768, "connection input too large");
            let value: Value =
                serde_json::from_str(&input).context("expected model and api_key JSON on stdin")?;
            let connection = api
                .request(Method::POST, "v1/pi/inference-connections", Some(value))
                .await?;
            let catalog = catalog(api).await?;
            let path = defaults_path(api, &catalog)?;
            let mut defaults = catalog["defaults"].clone();
            defaults["pi_connection_id"] = connection["id"].clone();
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent().context("preferences directory missing")?,
            )?;
            serde_json::to_writer(&mut file, &defaults)?;
            file.flush()?;
            file.as_file().sync_all()?;
            file.persist(path)
                .context("saving connected Pi inference")?;
            connection
        }
        Commands::Connections => {
            api.request(Method::GET, "v1/gateway-inference-connections", None)
                .await?
        }
        Commands::Disconnect { id } => {
            api.request(
                Method::POST,
                &format!("v1/gateway-inference-connections/{id}/revoke"),
                Some(json!({})),
            )
            .await?
        }
        Commands::Status { id } => {
            api.request(Method::GET, &format!("v1/pi/workspaces/{id}/session"), None)
                .await?
        }
        Commands::Stop { id } => {
            api.request(
                Method::POST,
                &format!("v1/pi/workspaces/{id}/stop"),
                Some(json!({})),
            )
            .await?
        }
        Commands::Restart { id } => {
            api.request(
                Method::POST,
                &format!("v1/pi/workspaces/{id}/restart"),
                Some(json!({})),
            )
            .await?
        }
        Commands::Attach { id } => return attach(api, id).await,
        Commands::Terminate { id } => {
            api.request(
                Method::POST,
                &format!("v1/pi/workspaces/{id}/terminate"),
                Some(json!({})),
            )
            .await?
        }
        Commands::Inference { model } => {
            api.request(
                Method::POST,
                "v1/pi/inference",
                Some(json!({"model_id":model})),
            )
            .await?
        }
        Commands::ReportError { operation } => {
            crate::telemetry::report(
                "pi-extension",
                &operation,
                &anyhow::anyhow!("Pi extension operation failed"),
            );
            json!({"reported":true})
        }
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

async fn run_local(api: &Api, command: Commands) -> Result<()> {
    let result = match command {
        Commands::Catalog => api.request(Method::GET, "v1/pi/catalog", None).await?,
        Commands::Defaults { model: Some(model), use_pi: false } => api.request(Method::POST, "v1/pi/defaults", Some(json!({"model_id":model}))).await?,
        Commands::List => api.request(Method::GET, "v1/pi/workspaces", None).await?,
        Commands::LocalCreate { provider, model, idempotency_key } => {
            let id = idempotency_key.unwrap_or_else(Uuid::new_v4);
            eprintln!("Creating local Pi session {id}");
            api.request(Method::POST, "v1/pi/workspaces", Some(json!({"provider":provider,"model_id":model,"idempotency_key":id}))).await?
        }
        Commands::Status { id } => api.request(Method::GET, &format!("v1/pi/workspaces/{id}/session"), None).await?,
        Commands::Stop { id } => api.request(Method::POST, &format!("v1/pi/workspaces/{id}/stop"), Some(json!({}))).await?,
        Commands::Restart { id } => api.request(Method::POST, &format!("v1/pi/workspaces/{id}/restart"), Some(json!({}))).await?,
        Commands::Terminate { id } => api.request(Method::POST, &format!("v1/pi/workspaces/{id}/terminate"), Some(json!({}))).await?,
        Commands::Attach { id } => return attach(api, id).await,
        _ => bail!("use /magic default to choose local inference and /magic new to choose the sandbox backend"),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

pub(crate) fn defaults_path(api: &Api, catalog: &Value) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let organization: Uuid = catalog["scope"]["organization_id"]
        .as_str()
        .context("organization missing")?
        .parse()?;
    let user: Uuid = catalog["scope"]["user_id"]
        .as_str()
        .context("account missing")?
        .parse()?;
    let key = Sha256::digest(format!("{}|{organization}|{user}", api.base).as_bytes());
    let directory = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
        .join(".config/magicmtn/cli");
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("defaults-{key:x}.json")))
}

async fn catalog(api: &Api) -> Result<Value> {
    let mut catalog = api.request(Method::GET, "v1/pi/catalog", None).await?;
    let saved = match fs::read(defaults_path(api, &catalog)?) {
        Ok(bytes) => Some(
            serde_json::from_slice::<Value>(&bytes).context("invalid saved workspace defaults")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading workspace defaults"),
    };
    if let Some(saved) = saved {
        if let Some(connection) = saved.get("pi_connection_id") {
            catalog["defaults"]["pi_connection_id"] = connection.clone();
        }
        catalog["defaults"]["inference_source"] = saved
            .get("inference_source")
            .cloned()
            .unwrap_or(json!("magicmtn"));
        if catalog["models"]
            .as_array()
            .is_some_and(|models| models.iter().any(|m| m["id"] == saved["model_id"]))
        {
            catalog["defaults"]["model_id"] = saved["model_id"].clone();
        }
    }
    Ok(catalog)
}

pub(crate) async fn managed_creation_body(
    api: &Api,
    model: Option<Uuid>,
    pi_connection: Option<Uuid>,
    key: Uuid,
) -> Result<Value> {
    let mut body = json!({"idempotency_key": key});
    if let Some(model) = model {
        body["model_id"] = json!(model);
    } else if let Some(connection) = pi_connection {
        body["pi_connection_id"] = json!(connection);
    } else {
        let catalog = catalog(api).await?;
        let defaults = &catalog["defaults"];
        if defaults["inference_source"] == "pi" {
            let connection: Uuid = serde_json::from_value(defaults["pi_connection_id"].clone())
                .context(
                "Connect your Pi inference with /magic new, or select --model or --pi-connection",
            )?;
            body["pi_connection_id"] = json!(connection);
        } else {
            let model: Uuid = serde_json::from_value(defaults["model_id"].clone())
                .context("Select an available inference model with /magic default or --model")?;
            body["model_id"] = json!(model);
        }
    }
    crate::configuration::creation_body(body)
}
async fn attach(api: &Api, id: Uuid) -> Result<()> {
    terminal(
        api,
        &format!("v1/pi/workspaces/{id}/terminal"),
        &format!("workspace {id}"),
    )
    .await
}

pub(crate) async fn sandbox_shell(api: &Api, id: &str) -> Result<()> {
    terminal(
        api,
        &format!("v1/sandboxes/{id}/terminal"),
        &format!("sandbox {id}"),
    )
    .await
}

pub(crate) async fn terminal(api: &Api, path: &str, label: &str) -> Result<()> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "this command requires an interactive terminal"
    );
    let (cols, rows) = crossterm::terminal::size()?;
    eprintln!("Connecting {label} (Ctrl+] disconnects; the sandbox remains running)…");
    let attached = api
        .request(Method::POST, path, Some(json!({"cols":cols,"rows":rows,"error_reporting":crate::configuration::error_reporting()?})))
        .await?;
    // WebSocket upgrades require HTTP/1.1 at TLS negotiation too. A request's
    // version alone does not prevent ALPN from choosing HTTP/2 and dropping
    // the Connection/Upgrade headers at the HTTPS ingress.
    let gateway_client = crate::http_client_builder()?.http1_only().build()?;
    let gateway = attached["poll_url"]
        .as_str()
        .map(url_for_gateway)
        .transpose()?;
    let attachment_token = attached["token"].as_str();
    ensure!(
        gateway.is_some() == attachment_token.is_some(),
        "incomplete gateway attachment"
    );
    let client_id = attached["client_id"]
        .as_str()
        .context("terminal client ID missing")?;
    let mut cursor = attached["cursor"]
        .as_u64()
        .context("terminal cursor missing")?;
    crossterm::terminal::enable_raw_mode()?;
    let _terminal = DropGuard::new((), |()| {
        // Remote pi can still be running when we detach. Restore the keyboard,
        // mouse and paste modes that its normal exit would have released.
        let _ = std::io::stdout().write_all(b"\x1b[?2004l\x1b[<u\x1b[>4;0m\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1006l\x1b[?2031l\x1b[?2026l\x1b[0m");
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = crossterm::terminal::disable_raw_mode();
    });
    // A separate screen keeps the suspended local pi display intact.
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    // Native thread: a blocked stdin read must not keep the Tokio runtime alive
    // after detach. Bounded buffering also prevents unlimited pasted input.
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            let mut buffer = vec![0; 4096];
            match input.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    let _ = sender.blocking_send(vec![]);
                    break;
                }
                Ok(n) => {
                    buffer.truncate(n);
                    if sender.blocking_send(buffer).is_err() {
                        break;
                    }
                }
            }
        }
    });
    if let Some(stream) = attached["stream_url"].as_str() {
        return streaming::attach(
            gateway_client,
            url_for_gateway(stream)?,
            gateway,
            attachment_token.context("terminal stream token missing")?,
            client_id,
            cursor,
            receiver,
        )
        .await;
    }
    let mut seq = 0_u64;
    let mut input_tail = Vec::new();
    let mut pending_input = Vec::new();
    let mut stdout = std::io::stdout();
    loop {
        let mut input = std::mem::take(&mut pending_input);
        while input.len() < 20 * 1024 {
            match receiver.try_recv() {
                Ok(bytes) if bytes.is_empty() => return Ok(()),
                Ok(bytes) => input.extend(bytes),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut inspected = input_tail;
        inspected.extend(&input);
        if detach_requested(&inspected) {
            return Ok(());
        }
        input_tail = inspected[inspected.len().saturating_sub(32)..].to_vec();
        seq += 1;
        let (cols, rows) = crossterm::terminal::size()?;
        let body = json!({"client_id":client_id,"seq":seq,"input":B64.encode(input),"cursor":cursor,"cols":cols,"rows":rows});
        let poll_path = format!("{path}/poll");
        let started = Instant::now();
        let response = loop {
            let request = if let Some(url) = &gateway {
                gateway_client
                    .post(url.clone())
                    .bearer_auth(attachment_token.unwrap())
            } else {
                api.authenticated(api.client.post(api.base.join(&poll_path)?))
            };
            let result = request
                .timeout(Duration::from_secs(5))
                .json(&body)
                .send()
                .await;
            match result {
                Ok(response) if response.status().is_success() => {
                    break response.json::<Value>().await?
                }
                Ok(response)
                    if response.status().is_client_error()
                        && response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS =>
                {
                    bail!("terminal disconnected: {}", response.text().await?)
                }
                _ if started.elapsed() > Duration::from_secs(15) => {
                    bail!("terminal connection lost; reconnect to {label}")
                }
                _ => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        };
        if response["lost"] == true {
            stdout.write_all(b"\r\n[Some earlier terminal output is no longer buffered.]\r\n")?;
        }
        let output = B64.decode(
            response["output"]
                .as_str()
                .context("terminal output missing")?,
        )?;
        stdout.write_all(&output)?;
        stdout.flush()?;
        cursor = response["cursor"]
            .as_u64()
            .context("terminal cursor missing")?;
        if let Some(code) = response["exit_code"].as_i64() {
            if code != 0 {
                bail!("remote terminal exited with status {code}");
            }
            return Ok(());
        }
        if output.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(60)) => {},
                bytes = receiver.recv() => match bytes {
                    Some(bytes) if !bytes.is_empty() => pending_input = bytes,
                    _ => return Ok(()),
                },
            }
        }
    }
}

// Match legacy, Kitty and xterm modifyOtherKeys encodings of Ctrl+]. A short
// tail is retained by attach() because terminal sequences can span reads.
fn detach_requested(bytes: &[u8]) -> bool {
    bytes.contains(&0x1d)
        || [
            b"\x1b[93;5u".as_slice(),
            b"\x1b[93;5:1u",
            b"\x1b[93;5:2u",
            b"\x1b[27;5;93~",
        ]
        .iter()
        .any(|sequence| {
            bytes
                .windows(sequence.len())
                .any(|window| window == *sequence)
        })
}
fn url_for_gateway(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))),
        "gateway attachment requires HTTPS"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "invalid gateway URL"
    );
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extension_records_the_installing_executable_as_a_javascript_string() -> Result<()> {
        let root = tempfile::tempdir()?;
        let executable = root.path().join("bin with spaces/ma\"gic");
        write_extension(root.path(), &executable)?;
        let source = fs::read_to_string(root.path().join("index.js"))?;
        assert!(source.contains(&format!(
            "const magicExecutable = {};",
            serde_json::to_string(&executable)?
        )));
        assert!(!source.contains("const magicExecutable = null;"));
        Ok(())
    }

    #[test]
    fn creating_a_workspace_requires_a_magicmtn_model() {
        use clap::Parser;
        let args = ["magic", "pi", "create"];
        assert_eq!(
            crate::Cli::try_parse_from(args).err().unwrap().kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
    #[test]
    fn detach_handles_terminal_keyboard_protocols() {
        for sequence in [
            b"\x1d".as_slice(),
            b"\x1b[93;5u",
            b"\x1b[93;5:1u",
            b"\x1b[27;5;93~",
        ] {
            assert!(detach_requested(sequence));
        }
        assert!(!detach_requested("ordinary input 雪".as_bytes()));
        assert!(!detach_requested(b"\x1b[93u"));
    }
}
