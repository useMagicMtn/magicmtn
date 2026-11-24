//! Workspace-scoped agent services and provider routing; ingress supplies identity.
pub use magicmtn_contracts::contracts;
pub use magicmtn_sdk::{InferenceBackend, SandboxBackend};
pub mod agent;
pub mod auth;
pub mod compute;
pub mod compute_runtime;
pub mod credential_vault;
pub mod image_registry;
pub mod model_runtime;
pub mod models;

pub mod library;
pub mod registry_scope;
pub mod sandbox_lifecycle;

pub mod inference_resources;
pub mod provider_connections;
pub mod provider_credentials;
pub mod provider_runners;
pub mod redaction;
pub mod workspace;

pub mod public_providers;

pub mod sandboxes;
pub mod schema;

pub mod sandbox_storage;
