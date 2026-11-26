//! Protocol v1: newline-delimited JSON RPC over a persistent process. Stdout is reserved
//! for the response; stderr is diagnostic and must never contain secrets.
use crate::{
    ExecResult, InferenceDeployment, InferenceDeploymentRef, InferenceDeploymentSpec,
    InferenceRequest, InferenceResponse, ProviderSandbox, ScpDirection, SearchRequest,
    SearchResponse,
};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CREDENTIAL_PROTOCOL_VERSION: u32 = 2;
pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxOperation {
    PrepareImage,
    Create,
    Status,
    Ssh,
    Terminate,
    Scp,
    Exec,
    ExecBounded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceDeploymentOperation {
    Create,
    Start,
    Status,
    Stop,
    Delete,
}

/// A selectable model supplied by a provider package, independent of account setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceModel {
    pub id: String,
    pub name: String,
    pub context_window: u32,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Account configuration, separate from workload operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<crate::provider_setup::SetupSpec>,
    /// Advertises web search through `SearchEngineBackend`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub search: bool,
    pub inference_tasks: Vec<String>,
    /// Provider-distributed model choices; no entry is selected automatically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inference_models: Vec<InferenceModel>,
    /// Optional v1 extension. Omitted for inference-only and older providers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inference_deployment_operations: Vec<InferenceDeploymentOperation>,
    pub sandbox_operations: Vec<SandboxOperation>,
    pub editable_surfaces: Vec<String>,
    pub observe: bool,
    pub evaluate: bool,
    pub deploy: bool,
}
impl Capabilities {
    pub fn sandbox(operations: Vec<SandboxOperation>) -> Self {
        Self {
            setup: None,
            search: false,
            inference_tasks: vec![],
            inference_models: vec![],
            inference_deployment_operations: vec![],
            sandbox_operations: operations,
            editable_surfaces: vec![],
            observe: false,
            evaluate: false,
            deploy: false,
        }
    }
    pub fn inference(tasks: Vec<String>) -> Self {
        Self {
            setup: None,
            search: false,
            inference_tasks: tasks,
            inference_models: vec![],
            inference_deployment_operations: vec![],
            sandbox_operations: vec![],
            editable_surfaces: vec![],
            observe: false,
            evaluate: false,
            deploy: false,
        }
    }
    pub fn search() -> Self {
        Self {
            search: true,
            ..Self::inference(vec![])
        }
    }
    pub fn with_inference_deployments(
        mut self,
        operations: Vec<InferenceDeploymentOperation>,
    ) -> Self {
        self.inference_deployment_operations = operations;
        self
    }
    pub fn supports(&self, operation: &Operation) -> bool {
        match operation {
            Operation::ProviderConfigure { .. } => self.setup.is_some(),
            Operation::SandboxPrepareImage { .. } => self
                .sandbox_operations
                .contains(&SandboxOperation::PrepareImage),
            Operation::Search { .. } => self.search,
            Operation::Describe | Operation::Shutdown => true,
            Operation::SandboxCreate { .. } => {
                self.sandbox_operations.contains(&SandboxOperation::Create)
            }
            Operation::SandboxStatus { .. } => {
                self.sandbox_operations.contains(&SandboxOperation::Status)
            }
            Operation::SandboxSsh { .. } => {
                self.sandbox_operations.contains(&SandboxOperation::Ssh)
            }
            Operation::SandboxTerminate { .. } => self
                .sandbox_operations
                .contains(&SandboxOperation::Terminate),
            Operation::SandboxScp { .. } => {
                self.sandbox_operations.contains(&SandboxOperation::Scp)
            }
            Operation::SandboxExec { .. } => {
                self.sandbox_operations.contains(&SandboxOperation::Exec)
            }
            Operation::SandboxExecBounded { .. } => self
                .sandbox_operations
                .contains(&SandboxOperation::ExecBounded),
            Operation::Infer { request } => self.inference_tasks.contains(&request.task),
            Operation::InferenceDeploymentCreate { .. } => self
                .inference_deployment_operations
                .contains(&InferenceDeploymentOperation::Create),
            Operation::InferenceDeploymentStart { .. } => self
                .inference_deployment_operations
                .contains(&InferenceDeploymentOperation::Start),
            Operation::InferenceDeploymentStatus { .. } => self
                .inference_deployment_operations
                .contains(&InferenceDeploymentOperation::Status),
            Operation::InferenceDeploymentStop { .. } => self
                .inference_deployment_operations
                .contains(&InferenceDeploymentOperation::Stop),
            Operation::InferenceDeploymentDelete { .. } => self
                .inference_deployment_operations
                .contains(&InferenceDeploymentOperation::Delete),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialSpec>,
    pub manifest_version: u32,
    pub provider_version: String,
    pub capabilities: Capabilities,
}

/// Required fields in the provider's JSON credential bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSpec {
    pub fields: Vec<String>,
}

impl CredentialSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut unique = std::collections::BTreeSet::new();
        anyhow::ensure!(
            !self.fields.is_empty()
                && self.fields.len() <= 32
                && self.fields.iter().all(|field| {
                    !field.is_empty()
                        && field.len() <= 96
                        && field
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        && unique.insert(field)
                }),
            "invalid provider credential fields"
        );
        Ok(())
    }

    pub fn check_values(&self, values: &CredentialValues) -> anyhow::Result<()> {
        self.validate()?;
        for field in &self.fields {
            anyhow::ensure!(
                values.0.get(field).is_some_and(|value| !value.is_empty()),
                "provider credential requires {field}"
            );
        }
        anyhow::ensure!(
            values.0.len() == self.fields.len(),
            "unexpected provider credential fields"
        );
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct CredentialValues(pub std::collections::BTreeMap<String, String>);

impl<'de> Deserialize<'de> for CredentialValues {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueFields;
        impl<'de> serde::de::Visitor<'de> for UniqueFields {
            type Value = CredentialValues;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a credential object with unique string fields")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut values = CredentialValues(Default::default());
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if values.0.contains_key(&key) {
                        return Err(serde::de::Error::custom(
                            "duplicate provider credential field",
                        ));
                    }
                    values.0.insert(key, value);
                }
                Ok(values)
            }
        }
        deserializer.deserialize_map(UniqueFields)
    }
}

impl Drop for CredentialValues {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

impl std::fmt::Debug for CredentialValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialValues([REDACTED])")
    }
}

/// Host-to-provider startup frame. It is separate from ordinary operation payloads.
#[derive(Serialize)]
pub struct ProviderStartup<'a> {
    pub id: u64,
    pub protocol_version: u32,
    pub provider: &'a str,
    pub credential: Option<&'a CredentialValues>,
    pub verify_account: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderStartupInput {
    pub id: u64,
    pub protocol_version: u32,
    pub provider: String,
    pub credential: Option<CredentialValues>,
    pub verify_account: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub protocol_version: u32,
    pub provider: String,
    pub operation: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "name", content = "arguments", rename_all = "snake_case")]
pub enum Operation {
    ProviderConfigure {
        options: std::collections::BTreeMap<String, String>,
    },
    Describe,
    Shutdown,
    SandboxPrepareImage {
        images: std::collections::BTreeMap<String, crate::image_registry::Image>,
    },
    Search {
        request: SearchRequest,
    },
    SandboxCreate {
        template: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_seconds: Option<u32>,
    },
    SandboxStatus {
        sandbox: ProviderSandbox,
    },
    SandboxSsh {
        sandbox: ProviderSandbox,
    },
    SandboxTerminate {
        sandbox: ProviderSandbox,
    },
    SandboxScp {
        sandbox: ProviderSandbox,
        local: String,
        remote: String,
        direction: ScpDirection,
    },
    SandboxExec {
        sandbox: ProviderSandbox,
        command: String,
        args: Vec<String>,
    },
    SandboxExecBounded {
        sandbox: ProviderSandbox,
        command: String,
        args: Vec<String>,
        timeout_ms: u64,
        max_output_bytes: usize,
    },
    Infer {
        request: InferenceRequest,
    },
    InferenceDeploymentCreate {
        spec: InferenceDeploymentSpec,
    },
    InferenceDeploymentStart {
        resource: InferenceDeploymentRef,
    },
    InferenceDeploymentStatus {
        resource: InferenceDeploymentRef,
    },
    InferenceDeploymentStop {
        resource: InferenceDeploymentRef,
    },
    InferenceDeploymentDelete {
        resource: InferenceDeploymentRef,
    },
}

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SandboxPrepareImage { .. } => "sandbox_prepare_image",
            Self::Search { .. } => "search",
            Self::ProviderConfigure { .. } => "provider_configure",
            Self::Describe => "describe",
            Self::Shutdown => "shutdown",
            Self::SandboxCreate { .. } => "sandbox_create",
            Self::SandboxStatus { .. } => "sandbox_status",
            Self::SandboxSsh { .. } => "sandbox_ssh",
            Self::SandboxTerminate { .. } => "sandbox_terminate",
            Self::SandboxScp { .. } => "sandbox_scp",
            Self::SandboxExec { .. } => "sandbox_exec",
            Self::SandboxExecBounded { .. } => "sandbox_exec_bounded",
            Self::Infer { .. } => "infer",
            Self::InferenceDeploymentCreate { .. } => "inference_deployment_create",
            Self::InferenceDeploymentStart { .. } => "inference_deployment_start",
            Self::InferenceDeploymentStatus { .. } => "inference_deployment_status",
            Self::InferenceDeploymentStop { .. } => "inference_deployment_stop",
            Self::InferenceDeploymentDelete { .. } => "inference_deployment_delete",
        }
    }
}

#[cfg(test)]
mod ttl_tests {
    use super::Operation;
    use serde_json::json;

    #[test]
    fn image_preparation_is_an_explicit_capability_and_pins_platform_images() {
        use super::{Capabilities, SandboxOperation};
        let frame = json!({"name":"sandbox_prepare_image","arguments":{"images":{"linux/amd64":{"repository":"example/image","digest":format!("sha256:{}","a".repeat(64))}}}});
        let operation: Operation = serde_json::from_value(frame.clone()).unwrap();
        assert_eq!(operation.kind(), "sandbox_prepare_image");
        assert!(!Capabilities::sandbox(vec![SandboxOperation::Create]).supports(&operation));
        assert!(Capabilities::sandbox(vec![SandboxOperation::PrepareImage]).supports(&operation));
        assert_eq!(serde_json::to_value(operation).unwrap(), frame);
    }

    #[test]
    fn sandbox_create_keeps_old_frames_and_carries_ttl() {
        let old = json!({"name":"sandbox_create","arguments":{"template":"image:example|session"}});
        let decoded: Operation = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(
            decoded,
            Operation::SandboxCreate {
                ttl_seconds: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(decoded).unwrap(), old);

        let with_ttl = json!({"name":"sandbox_create","arguments":{"template":"image:example|session","ttl_seconds":1800}});
        let decoded: Operation = serde_json::from_value(with_ttl.clone()).unwrap();
        assert!(matches!(
            decoded,
            Operation::SandboxCreate {
                ttl_seconds: Some(1800),
                ..
            }
        ));
        assert_eq!(serde_json::to_value(decoded).unwrap(), with_ttl);
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub id: u64,
    pub protocol_version: u32,
    pub provider: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedOperation,
    UpstreamUnavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderError {
    pub code: ErrorCode,
    pub retryable: bool,
    pub message: String,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Value),
    Error(ProviderError),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    ConfiguredConnection(crate::provider_setup::ConfiguredConnection),
    PreparedImage(crate::image_registry::PreparedImage),
    Description(ProviderManifest),
    VerifiedAccount(String),
    Search(SearchResponse),
    Sandbox(ProviderSandbox),
    SandboxStatus(crate::SandboxState),
    Inference(InferenceResponse),
    InferenceDeployment(InferenceDeployment),
    Text(String),
    Exec(ExecResult),
    Done,
}
