//! A local, outbound-only provider runner. No provider package is downloaded or
//! installed on demand; the local catalog pins everything this agent can run.
use anyhow::{ensure, Context, Result};
use clap::Parser;
use magicmtn_provider_framework::host::{Pin, ProcessEnvironment, Registry};
use magicmtn_sdk::plugin::{ErrorCode, Operation, Outcome, ProviderError};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Parser)]
pub(crate) struct Args {
    #[arg(long, env = "MAGICMTN_SERVER_URL")]
    pub(crate) server_url: String,
    #[arg(long, env = "MAGICMTN_PROVIDER_CATALOG")]
    pub(crate) catalog: PathBuf,
}
async fn post(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
    body: Value,
) -> Result<Value> {
    Ok(client
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub(crate) async fn run(args: Args) -> Result<!> {
    let url = reqwest::Url::parse(&args.server_url)?;
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "runner requires HTTPS or loopback HTTP"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid server URL"
    );
    let token = std::env::var("MAGICMTN_PROVIDER_RUNNER_TOKEN")
        .context("supply the runner credential through MAGICMTN_PROVIDER_RUNNER_TOKEN")?;
    let catalog = args.catalog.canonicalize()?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(
            catalog
                .parent()
                .context("catalog directory missing")?
                .join(".lock"),
        )?;
    lock.try_lock_shared()
        .context("provider store is being modified; try starting the runner again")?;
    let registry = Registry::from_catalog(&catalog)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = args.server_url.trim_end_matches('/').to_owned();
    let heartbeat_client = client.clone();
    let heartbeat_base = base.clone();
    let heartbeat_token = token.clone();
    tokio::spawn(async move {
        loop {
            let _ = post(
                &heartbeat_client,
                &heartbeat_base,
                &heartbeat_token,
                "/v1/provider-runner/heartbeat",
                json!({}),
            )
            .await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
    loop {
        let response = match post(
            &client,
            &base,
            &token,
            "/v1/provider-runner/claim",
            json!({}),
        )
        .await
        {
            Ok(value) => value,
            Err(_) => {
                eprintln!("provider runner: server unavailable");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };
        let job = &response["job"];
        if job.is_null() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let result = async {
            let pin: Pin = serde_json::from_value(job["binding"]["pin"].clone())?;
            ensure!(
                registry.installed_pin(&pin)?.manifest.credential.is_none(),
                "credential-aware providers require an authorized execution host"
            );
            let operation: Operation = serde_json::from_value(job["operation"].clone())?;
            let mut values: BTreeMap<String, String> =
                serde_json::from_value(job["binding"]["configuration"].clone())?;
            values.insert(
                "PATH".into(),
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
            );
            // Local provider runtimes live on this host. The server sends no keys.
            for key in [
                "HOME",
                "MAGICMTN_MXC_STATE_DIR",
                "MAGICMTN_FIRECRACKER_CONFIG",
            ] {
                if let Ok(value) = std::env::var(key) {
                    values.insert(key.into(), value);
                }
            }
            let scope = job["binding"]["connection_id"]
                .as_str()
                .context("connection missing")?;
            let result = registry
                .call_configured(
                    &pin,
                    scope,
                    operation,
                    Duration::from_secs(300),
                    Some(&ProcessEnvironment { values }),
                )
                .await;
            registry.release(scope).await;
            result
        }
        .await;
        let outcome = match result {
            Ok(value) => Outcome::Ok(value),
            Err(_) => Outcome::Error(ProviderError {
                code: ErrorCode::UpstreamUnavailable,
                retryable: false,
                message: "local provider operation failed; reconcile before retry".into(),
            }),
        };
        // Never rerun a claimed operation if posting its result fails.
        let id = job["id"].as_str().context("job id missing")?;
        let path = format!("/v1/provider-runner/jobs/{id}/result");
        let body = json!({"lease_token":job["lease_token"],"outcome":outcome});
        loop {
            match post(&client, &base, &token, &path, body.clone()).await {
                Ok(_) => break,
                Err(error) => {
                    let status = error
                        .downcast_ref::<reqwest::Error>()
                        .and_then(reqwest::Error::status);
                    if status.is_some_and(|status| {
                        status.is_client_error()
                            && status != reqwest::StatusCode::REQUEST_TIMEOUT
                            && status != reqwest::StatusCode::TOO_MANY_REQUESTS
                    }) {
                        eprintln!("provider runner: result rejected by server");
                        break;
                    }
                    eprintln!("provider runner: retrying result delivery");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }
}
