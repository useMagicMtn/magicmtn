//! Shared persistent JSON RPC runner for independently packaged providers.
use anyhow::{anyhow, bail, Context, Result};
use magicmtn_sdk::plugin::{
    Capabilities, CredentialSpec, CredentialValues, ErrorCode, Operation, Outcome, ProviderError,
    ProviderManifest, ProviderStartupInput, Request, Response, Value, CREDENTIAL_PROTOCOL_VERSION,
    MANIFEST_VERSION, PROTOCOL_VERSION,
};
use magicmtn_sdk::{
    BoxFuture, InferenceBackend, InferenceDeploymentBackend, SandboxBackend, SearchEngineBackend,
};
use std::{
    io::{BufRead, Read, Write},
    path::Path,
};
pub const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

/// Serve web search requests from a persistent provider process.
pub async fn run_search(
    provider: &str,
    version: &str,
    backend: impl SearchEngineBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, Capabilities::search(), |operation| {
        Box::pin(async move {
            match operation {
                Operation::Search { request } => backend.search(&request).await.map(Value::Search),
                _ => Err(anyhow!("unsupported search operation")),
            }
        })
    })
    .await
}

async fn serve<'a>(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    dispatch: impl FnMut(Operation) -> BoxFuture<'a, Value>,
) -> Result<()> {
    let manifest = ProviderManifest {
        credential: None,
        manifest_version: MANIFEST_VERSION,
        provider_version: version.into(),
        capabilities,
    };
    serve_io(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        provider,
        PROTOCOL_VERSION,
        &manifest,
        dispatch,
    )
    .await
}

/// Start a credential-aware provider. A verification startup returns the stable
/// upstream account identifier. Normal startup constructs the client without a
/// separate verification request. Discovery runs without calling the initializer.
pub async fn run_credentialed<D, F>(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    spec: CredentialSpec,
    initialize: impl FnOnce(CredentialValues, bool) -> F,
) -> Result<()>
where
    D: FnMut(Operation) -> BoxFuture<'static, Value>,
    F: std::future::Future<Output = Result<(Option<String>, D)>>,
{
    spec.validate()?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let Some(bytes) = read_frame(&mut input)? else {
        return Ok(());
    };
    let startup: ProviderStartupInput =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid provider startup frame"))?;
    drop(bytes);
    ensure_startup(&startup, provider)?;
    let manifest = ProviderManifest {
        credential: Some(spec),
        manifest_version: MANIFEST_VERSION,
        provider_version: version.into(),
        capabilities,
    };
    let response = |outcome| Response {
        id: startup.id,
        protocol_version: CREDENTIAL_PROTOCOL_VERSION,
        provider: provider.into(),
        outcome,
    };
    let Some(credential) = startup.credential else {
        return write_response(
            &mut output,
            &response(Outcome::Ok(Value::Description(manifest))),
        );
    };
    let prepared = if manifest
        .credential
        .as_ref()
        .unwrap()
        .check_values(&credential)
        .is_ok()
    {
        initialize(credential, startup.verify_account).await.ok()
    } else {
        None
    };
    match prepared {
        Some((Some(account), _)) if startup.verify_account && !account.is_empty() => {
            write_response(
                &mut output,
                &response(Outcome::Ok(Value::VerifiedAccount(account))),
            )
        }
        Some((_, dispatch)) if !startup.verify_account => {
            write_response(
                &mut output,
                &response(Outcome::Ok(Value::Description(manifest.clone()))),
            )?;
            serve_io(
                &mut input,
                &mut output,
                provider,
                CREDENTIAL_PROTOCOL_VERSION,
                &manifest,
                dispatch,
            )
            .await
        }
        _ => write_response(
            &mut output,
            &response(Outcome::Error(ProviderError {
                code: ErrorCode::Internal,
                retryable: false,
                message: "provider credential initialization failed".into(),
            })),
        ),
    }
}

fn ensure_startup(startup: &ProviderStartupInput, provider: &str) -> Result<()> {
    anyhow::ensure!(
        startup.protocol_version == CREDENTIAL_PROTOCOL_VERSION
            && startup.provider == provider
            && startup.id == 1
            && (!startup.verify_account || startup.credential.is_some()),
        "provider startup protocol or identity mismatch"
    );
    Ok(())
}

fn read_frame(input: &mut impl BufRead) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    if input
        .take(MAX_MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut bytes)?
        == 0
    {
        return Ok(None);
    }
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_MESSAGE_BYTES && bytes.ends_with(b"\n"),
        "invalid or oversized RPC frame"
    );
    Ok(Some(bytes))
}

fn write_response(output: &mut impl Write, response: &Response) -> Result<()> {
    let bytes = serde_json::to_vec(response)?;
    anyhow::ensure!(
        (bytes.len() as u64) < MAX_MESSAGE_BYTES,
        "provider result exceeds message limit"
    );
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

async fn serve_io<'a>(
    input: &mut impl BufRead,
    output: &mut impl Write,
    provider: &str,
    protocol_version: u32,
    manifest: &ProviderManifest,
    mut dispatch: impl FnMut(Operation) -> BoxFuture<'a, Value>,
) -> Result<()> {
    while let Some(bytes) = read_frame(input)? {
        let request: Request = serde_json::from_slice(&bytes).context("invalid provider RPC")?;
        if request.protocol_version != protocol_version || request.provider != provider {
            bail!("provider protocol or identity mismatch");
        }
        let shutdown = matches!(request.operation, Operation::Shutdown);
        let unsupported = !manifest.capabilities.supports(&request.operation);
        let result = match request.operation {
            Operation::Describe => Ok(Value::Description(manifest.clone())),
            Operation::Shutdown => Ok(Value::Done),
            _ if unsupported => Err(anyhow!("unsupported operation")),
            operation => dispatch(operation).await,
        };
        let outcome = match result {
            Ok(value) => Outcome::Ok(value),
            Err(error) => Outcome::Error(
                error
                    .downcast_ref::<ProviderError>()
                    .cloned()
                    .unwrap_or(ProviderError {
                        code: if unsupported {
                            ErrorCode::UnsupportedOperation
                        } else {
                            ErrorCode::Internal
                        },
                        retryable: false,
                        message: if unsupported {
                            "operation is not supported"
                        } else {
                            "provider operation failed"
                        }
                        .into(),
                    }),
            ),
        };
        write_response(
            output,
            &Response {
                id: request.id,
                protocol_version,
                provider: provider.into(),
                outcome,
            },
        )?;
        if shutdown {
            break;
        }
    }
    Ok(())
}
pub async fn run_sandbox(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl SandboxBackend + Sync,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::SandboxCreate {
                    template,
                    ttl_seconds,
                } => match ttl_seconds {
                    Some(0) => Err(anyhow!("sandbox TTL must be positive")),
                    Some(seconds) => backend.create_with_ttl(&template, seconds).await,
                    None => backend.create(&template).await,
                }
                .map(Value::Sandbox),
                Operation::SandboxStatus { sandbox } => {
                    backend.status(&sandbox).await.map(Value::SandboxStatus)
                }
                Operation::SandboxTerminate { sandbox } => {
                    backend.terminate(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxExec {
                    sandbox,
                    command,
                    args,
                } => backend
                    .exec(&sandbox, &command, &args)
                    .await
                    .map(Value::Text),
                Operation::SandboxExecBounded {
                    sandbox,
                    command,
                    args,
                    timeout_ms,
                    max_output_bytes,
                } => backend
                    .exec_bounded(&sandbox, &command, &args, timeout_ms, max_output_bytes)
                    .await
                    .map(Value::Exec),
                Operation::SandboxSsh { sandbox } => {
                    backend.ssh(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxScp {
                    sandbox,
                    local,
                    remote,
                    direction,
                } => backend
                    .scp(&sandbox, Path::new(&local), Path::new(&remote), direction)
                    .await
                    .map(|()| Value::Done),
                _ => Err(anyhow!("unsupported sandbox operation")),
            }
        })
    })
    .await
}
pub async fn run_inference(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl InferenceBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                _ => Err(anyhow!("unsupported inference operation")),
            }
        })
    })
    .await
}

/// Serve sandbox and inference operations from one provider process.
pub async fn run_sandbox_with_inference(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl SandboxBackend + InferenceBackend + Sync,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::SandboxCreate {
                    template,
                    ttl_seconds,
                } => match ttl_seconds {
                    Some(0) => Err(anyhow!("sandbox TTL must be positive")),
                    Some(seconds) => backend.create_with_ttl(&template, seconds).await,
                    None => backend.create(&template).await,
                }
                .map(Value::Sandbox),
                Operation::SandboxStatus { sandbox } => {
                    backend.status(&sandbox).await.map(Value::SandboxStatus)
                }
                Operation::SandboxTerminate { sandbox } => {
                    backend.terminate(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxExec {
                    sandbox,
                    command,
                    args,
                } => backend
                    .exec(&sandbox, &command, &args)
                    .await
                    .map(Value::Text),
                Operation::SandboxExecBounded {
                    sandbox,
                    command,
                    args,
                    timeout_ms,
                    max_output_bytes,
                } => backend
                    .exec_bounded(&sandbox, &command, &args, timeout_ms, max_output_bytes)
                    .await
                    .map(Value::Exec),
                Operation::SandboxSsh { sandbox } => {
                    backend.ssh(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxScp {
                    sandbox,
                    local,
                    remote,
                    direction,
                } => backend
                    .scp(&sandbox, Path::new(&local), Path::new(&remote), direction)
                    .await
                    .map(|()| Value::Done),
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                _ => Err(anyhow!("unsupported operation")),
            }
        })
    })
    .await
}
#[cfg(feature = "host")]
pub mod host;

/// Serve invocation and optional lifecycle capabilities from one provider.
/// Shutdown closes the RPC process only; remote deployments outlive it.
pub async fn run_inference_with_deployments(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl InferenceBackend + InferenceDeploymentBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                Operation::InferenceDeploymentCreate { spec } => {
                    backend.create(&spec).await.map(Value::InferenceDeployment)
                }
                Operation::InferenceDeploymentStart { resource } => backend
                    .start(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentStatus { resource } => backend
                    .status(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentStop { resource } => backend
                    .stop(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentDelete { resource } => {
                    backend.delete(&resource).await.map(|()| Value::Done)
                }
                _ => Err(anyhow!("unsupported inference operation")),
            }
        })
    })
    .await
}
