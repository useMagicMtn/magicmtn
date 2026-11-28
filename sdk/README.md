# MagicMtn provider SDK

The Rust provider SDK defines five backend interfaces for connecting sandbox compute, model inference, inference deployments, image registries, and web search to MagicMtn. Implement the interfaces your provider supports in the [`magicmtn-sdk`](rust/magicmtn-sdk) crate. A provider can implement multiple interfaces.

| Interface                    | Responsibility                      | Definition                                                                 |
| ---------------------------- | ----------------------------------- | -------------------------------------------------------------------------- |
| `SandboxBackend`             | Create and operate sandboxes        | [`lib.rs`](rust/magicmtn-sdk/src/lib.rs)                                   |
| `InferenceBackend`           | Invoke local or hosted models       | [`inference.rs`](rust/magicmtn-sdk/src/inference.rs)                       |
| `InferenceDeploymentBackend` | Manage inference serving resources  | [`inference_deployment.rs`](rust/magicmtn-sdk/src/inference_deployment.rs) |
| `ImageRegistryBackend`       | Store and retrieve container images | [`image_registry.rs`](rust/magicmtn-sdk/src/image_registry.rs)             |
| `SearchEngineBackend`        | Search the web for ranked sources   | [`search_engine.rs`](rust/magicmtn-sdk/src/search_engine.rs)               |
