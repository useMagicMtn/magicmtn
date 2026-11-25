//! Public Rust contract for MagicMtn provider executables.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Provider-owned state. The tenant service assigns its own record ID, timestamps,
/// notes, template name, and authorization context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSandbox {
    pub provider_id: String,
    pub status: Option<String>,
    pub create_time_ms: Option<i64>,
    pub metadata: Option<Value>,
}

/// An authoritative observation, never inferred from a transport error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Running,
    Stopped,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScpDirection {
    ToSandbox,
    FromSandbox,
}

pub type ProviderResult<T> = Result<T>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u64,
    pub timed_out: bool,
    pub truncated: bool,
}

pub trait SandboxBackend {
    /// Materialize and validate an image for this provider's sandbox platform.
    fn prepare_image<'a>(
        &'a self,
        _images: &'a std::collections::BTreeMap<String, image_registry::Image>,
    ) -> BoxFuture<'a, image_registry::PreparedImage> {
        Box::pin(async { anyhow::bail!("this provider does not support image preparation") })
    }
    fn create<'a>(&'a self, template: &'a str) -> BoxFuture<'a, ProviderSandbox>;
    /// Maximum sandbox lifetime in seconds. Providers without a native lifetime
    /// control may use the service's durable expiry and termination worker.
    fn create_with_ttl<'a>(
        &'a self,
        template: &'a str,
        _ttl_seconds: u32,
    ) -> BoxFuture<'a, ProviderSandbox> {
        self.create(template)
    }
    fn status<'a>(&'a self, _sandbox: &'a ProviderSandbox) -> BoxFuture<'a, SandboxState> {
        Box::pin(async {
            Err(plugin::ProviderError {
                code: plugin::ErrorCode::UnsupportedOperation,
                retryable: false,
                message: "sandbox status unsupported".into(),
            }
            .into())
        })
    }
    /// Import a complete OCI image layout and start a sandbox from it.
    ///
    /// The layout has one root descriptor (possibly a multi-platform index).
    /// The backend selects a compatible platform or fails explicitly. Registry
    /// credentials are not needed here. Existing template creation is separate.
    fn create_from_image<'a>(&'a self, _image: &'a Path) -> BoxFuture<'a, ProviderSandbox> {
        Box::pin(async { anyhow::bail!("creating a sandbox from an OCI layout is unsupported") })
    }
    fn ssh<'a>(&'a self, sandbox: &'a ProviderSandbox) -> BoxFuture<'a, ()>;
    /// Idempotent removal: an already absent resource is a successful result.
    fn terminate<'a>(&'a self, sandbox: &'a ProviderSandbox) -> BoxFuture<'a, ()>;
    fn scp<'a>(
        &'a self,
        sandbox: &'a ProviderSandbox,
        local: &'a Path,
        remote: &'a Path,
        direction: ScpDirection,
    ) -> BoxFuture<'a, ()>;
    fn exec<'a>(
        &'a self,
        sandbox: &'a ProviderSandbox,
        command: &'a str,
        args: &'a [String],
    ) -> BoxFuture<'a, String>;
    fn exec_bounded<'a>(
        &'a self,
        _sandbox: &'a ProviderSandbox,
        _command: &'a str,
        _args: &'a [String],
        _timeout_ms: u64,
        _max_output_bytes: usize,
    ) -> BoxFuture<'a, ExecResult> {
        Box::pin(async { anyhow::bail!("bounded exec unsupported") })
    }
}

pub mod inference;
pub use inference::{InferenceBackend, InferenceRequest, InferenceResponse};
pub mod inference_deployment;
pub use inference_deployment::{
    InferenceDeployment, InferenceDeploymentBackend, InferenceDeploymentRef,
    InferenceDeploymentSpec, InferenceDeploymentState,
};
pub mod image_registry;
pub use image_registry::{Image, ImageRegistryBackend};
pub mod search_engine;
pub use search_engine::{SearchEngineBackend, SearchRequest, SearchResponse, SearchResult};
pub mod plugin;

/// Shared chat task conventions and compatibility helpers.
pub mod chat;

/// Provider account setup and connection verification.
pub mod provider_setup;
