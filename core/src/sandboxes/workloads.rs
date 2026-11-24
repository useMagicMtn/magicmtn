//! Bounded workload execution in managed sandboxes.
use super::*;
use crate::provider_connections::{self as connections, Binding};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Execute {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    timeout_ms: u64,
    max_output_bytes: usize,
}
impl Execute {
    fn validate(&self) -> Result<(), (StatusCode, String)> {
        if self.command.is_empty()
            || self.command.len() > 256
            || self.command.starts_with('-')
            || self.command.contains('\0')
            || self
                .args
                .iter()
                .any(|arg| arg.contains('\0') || arg.len() > 2048)
            || self.command.len() + self.args.iter().map(String::len).sum::<usize>() > 128 * 1024
            || self.args.len() > 32
            || !(1..=30_000).contains(&self.timeout_ms)
            || !(1..=16 * 1024).contains(&self.max_output_bytes)
        {
            return Err(invalid(
                "invalid command or execution limits (maximum 30 seconds, 16 KiB output, 32 arguments of 2 KiB each)",
            ));
        }
        Ok(())
    }
}

fn exec_rpc_timeout(nitro: bool, command_timeout_ms: u64) -> Duration {
    // Nitro's provider adapter reserves 45 seconds for startup and 40 for command transport.
    if nitro {
        Duration::from_secs(110)
    } else {
        Duration::from_millis(command_timeout_ms + 30_000)
    }
}

pub(super) async fn execute(
    State(s): State<ServiceState>,
    Path(id): Path<String>,
    Json(body): Json<Execute>,
) -> Reply {
    body.validate()?;
    let saved = resolve_store(&s.store, &id).await?;
    let sandbox = super::execution_sandbox(&saved)?;
    let binding: Binding =
        serde_json::from_value(saved["metadata"]["_magicmtn_connection"].clone())
            .map_err(failed)?;
    let nitro = binding.pin.name == "aws" && sandbox.provider_id.starts_with("nitro-eks/");
    let timeout = exec_rpc_timeout(nitro, body.timeout_ms);
    let result = connections::dispatch_store(
        &s.store,
        &s.registry,
        &binding,
        &format!("sandbox:{}", saved["id"].as_str().unwrap()),
        Operation::SandboxExecBounded {
            sandbox,
            command: body.command,
            args: body.args,
            timeout_ms: body.timeout_ms,
            max_output_bytes: body.max_output_bytes,
        },
        timeout,
        false,
    )
    .await;
    match result {
        Ok(ProviderValue::Exec(result)) => Ok(Json(json!(result))),
        Err(error) if error.is::<connections::CredentialUnavailable>() => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "provider credential unavailable".into(),
        )),

        other => {
            if nitro {
                if let Some(message) = other
                    .as_ref()
                    .err()
                    .and_then(|error| error.downcast_ref::<magicmtn_sdk::plugin::ProviderError>())
                    .and_then(|error| {
                        magicmtn_provider_framework::host::nitro_startup_error("aws", error)
                    })
                {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("{message}; no command was submitted"),
                    ));
                }
            }
            tracing::error!("sandbox workload execution outcome uncertain");
            Err((StatusCode::BAD_GATEWAY, "Execution outcome uncertain; inspect the sandbox before retrying a command that changes state".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nitro_startup_budget_is_separate_from_command_timeout() {
        assert_eq!(exec_rpc_timeout(true, 1000), Duration::from_secs(110));
        assert_eq!(exec_rpc_timeout(true, 30_000), Duration::from_secs(110));
        assert_eq!(exec_rpc_timeout(false, 1000), Duration::from_secs(31));
    }

    #[test]
    fn execution_is_bounded_before_dispatch() {
        let mut body = Execute {
            command: "sh".into(),
            args: vec!["-lc".into(), "echo hello".into()],
            timeout_ms: 30_000,
            max_output_bytes: 16_384,
        };
        assert!(body.validate().is_ok());
        body.timeout_ms += 1;
        assert!(body.validate().is_err());
        body.timeout_ms = 1000;
        body.max_output_bytes = 16_385;
        assert!(body.validate().is_err());
        body.max_output_bytes = 16_384;
        body.args = vec!["x".repeat(2048); 32];
        assert!(body.validate().is_ok());
        body.args.push("overflow".into());
        assert!(body.validate().is_err());
        body.args = vec!["x".repeat(2049)];
        assert!(body.validate().is_err());
        body.args = vec!["bad\0arg".into()];
        assert!(body.validate().is_err());
    }
}
