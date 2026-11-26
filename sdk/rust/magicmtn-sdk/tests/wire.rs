use magicmtn_sdk::plugin::{Operation, Outcome, Request, Response, Value, PROTOCOL_VERSION};

#[test]
fn search_extension_preserves_legacy_manifests_and_negotiates_typed_results() {
    use magicmtn_sdk::plugin::Capabilities;
    use magicmtn_sdk::{SearchRequest, SearchResponse, SearchResult};

    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/manifest_response_v1.json")).unwrap();
    let response: Response = serde_json::from_value(golden.clone()).unwrap();
    let Outcome::Ok(Value::Description(manifest)) = &response.outcome else {
        panic!("manifest expected");
    };
    let operation = Operation::Search {
        request: SearchRequest::new("Rust async"),
    };
    assert!(!manifest.capabilities.supports(&operation));
    assert_eq!(serde_json::to_value(response).unwrap(), golden);
    assert!(Capabilities::search().supports(&operation));
    assert!(!Capabilities::inference(vec![]).supports(&operation));
    assert_eq!(operation.kind(), "search");
    let wire = serde_json::to_value(operation).unwrap();
    assert_eq!(wire["name"], "search");
    let decoded: Operation = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
    let response = SearchResponse {
        results: vec![SearchResult {
            url: "https://doc.rust-lang.org/book/".into(),
            title: "Rust".into(),
            content: "Source excerpt".into(),
        }],
    };
    let wire = serde_json::to_value(Value::Search(response.clone())).unwrap();
    assert_eq!(wire["kind"], "search");
    let Value::Search(decoded) = serde_json::from_value(wire).unwrap() else {
        panic!("search response expected");
    };
    assert_eq!(decoded, response);
}

#[test]
fn sandbox_status_is_negotiated_and_has_typed_results() {
    use magicmtn_sdk::{
        plugin::{Capabilities, SandboxOperation},
        ProviderSandbox, SandboxState,
    };
    let operation = Operation::SandboxStatus {
        sandbox: ProviderSandbox {
            provider_id: "test".into(),
            status: None,
            create_time_ms: None,
            metadata: None,
        },
    };
    assert!(!Capabilities::sandbox(vec![SandboxOperation::Terminate]).supports(&operation));
    assert!(Capabilities::sandbox(vec![SandboxOperation::Status]).supports(&operation));
    let wire = serde_json::to_value(&operation).unwrap();
    assert_eq!(wire["name"], "sandbox_status");
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<Operation>(wire.clone()).unwrap()).unwrap(),
        wire
    );
    for state in [
        SandboxState::Running,
        SandboxState::Stopped,
        SandboxState::Missing,
    ] {
        let value = serde_json::to_value(Value::SandboxStatus(state)).unwrap();
        let Value::SandboxStatus(decoded) = serde_json::from_value(value).unwrap() else {
            panic!("wrong value");
        };
        assert_eq!(decoded, state);
    }
}

#[test]
fn golden_v1_wire_round_trips_without_server_fields() {
    let request: Request =
        serde_json::from_str(include_str!("fixtures/describe_request_v1.json")).unwrap();
    assert_eq!(request.protocol_version, PROTOCOL_VERSION);
    assert!(matches!(request.operation, Operation::Describe));
    let response: Response =
        serde_json::from_str(include_str!("fixtures/create_response_v1.json")).unwrap();
    match response.outcome {
        Outcome::Ok(Value::Sandbox(sandbox)) => assert_eq!(sandbox.provider_id, "mock-123"),
        _ => panic!("wrong result"),
    }
    let response: Response =
        serde_json::from_str(include_str!("fixtures/manifest_response_v1.json")).unwrap();
    match response.outcome {
        Outcome::Ok(Value::Description(manifest)) => {
            assert_eq!(manifest.capabilities.sandbox_operations.len(), 3)
        }
        _ => panic!("wrong result"),
    }
    let malformed = r#"{"protocol_version":1,"provider":"mock","outcome":{"ok":{"kind":"sandbox","value":{"id":"00000000-0000-0000-0000-000000000000","provider_id":"x","status":null,"create_time_ms":null,"metadata":null}}}}"#;
    assert!(serde_json::from_str::<Response>(malformed).is_err());
}

#[test]
fn lifecycle_extension_preserves_legacy_manifests_and_gates_each_operation() {
    use magicmtn_sdk::plugin::{Capabilities, InferenceDeploymentOperation};
    use magicmtn_sdk::{InferenceDeploymentRef, InferenceDeploymentSpec};
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/manifest_response_v1.json")).unwrap();
    let response: Response = serde_json::from_value(golden.clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), golden);
    let reference = InferenceDeploymentRef {
        provider_id: "opaque-resource".into(),
    };
    let operations = [
        (
            InferenceDeploymentOperation::Create,
            Operation::InferenceDeploymentCreate {
                spec: InferenceDeploymentSpec {
                    name: "demo".into(),
                    model: "test".into(),
                    configuration: serde_json::json!({}),
                },
            },
        ),
        (
            InferenceDeploymentOperation::Start,
            Operation::InferenceDeploymentStart {
                resource: reference.clone(),
            },
        ),
        (
            InferenceDeploymentOperation::Status,
            Operation::InferenceDeploymentStatus {
                resource: reference.clone(),
            },
        ),
        (
            InferenceDeploymentOperation::Stop,
            Operation::InferenceDeploymentStop {
                resource: reference.clone(),
            },
        ),
        (
            InferenceDeploymentOperation::Delete,
            Operation::InferenceDeploymentDelete {
                resource: reference,
            },
        ),
    ];
    for (capability, operation) in &operations {
        let legacy = Capabilities::inference(vec!["chat".into()]);
        assert!(!legacy.supports(operation));
        let capabilities = legacy.with_inference_deployments(vec![capability.clone()]);
        for (other, candidate) in &operations {
            assert_eq!(capabilities.supports(candidate), other == capability);
        }
        let request = Request {
            id: 1,
            provider: "test".into(),
            protocol_version: PROTOCOL_VERSION,
            operation: serde_json::from_value(serde_json::to_value(operation).unwrap()).unwrap(),
        };
        let wire = serde_json::to_value(&request).unwrap();
        let decoded: Request = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
    }
}

#[test]
fn lifecycle_trait_allows_a_subset_without_changing_inference() {
    use magicmtn_sdk::{InferenceDeploymentBackend, InferenceDeploymentRef};
    struct External;
    impl InferenceDeploymentBackend for External {}
    // Defaults return a typed unsupported error, rather than pretend success.
    let resource = InferenceDeploymentRef {
        provider_id: "external".into(),
    };
    let mut future = External.stop(&resource);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let std::task::Poll::Ready(Err(error)) = future.as_mut().poll(&mut context) else {
        panic!("default lifecycle operation should immediately reject")
    };
    let error = error
        .downcast_ref::<magicmtn_sdk::plugin::ProviderError>()
        .unwrap();
    assert_eq!(
        error.code,
        magicmtn_sdk::plugin::ErrorCode::UnsupportedOperation
    );
    assert!(!error.retryable);
}

#[test]
fn provider_model_choices_are_optional_and_do_not_select_a_default() {
    use magicmtn_sdk::plugin::{Capabilities, InferenceModel};
    let legacy = Capabilities::inference(vec!["chat".into()]);
    let wire = serde_json::to_value(&legacy).unwrap();
    assert!(wire.get("inference_models").is_none());
    assert!(serde_json::from_value::<Capabilities>(wire)
        .unwrap()
        .inference_models
        .is_empty());
    let mut choices = legacy;
    choices.inference_models = vec![InferenceModel {
        id: "example/model".into(),
        name: "Example model".into(),
        context_window: 32768,
        max_tokens: 4096,
    }];
    let wire = serde_json::to_value(&choices).unwrap();
    assert_eq!(wire["inference_models"][0]["id"], "example/model");
    assert!(wire.get("default_model").is_none());
    assert_eq!(
        serde_json::from_value::<Capabilities>(wire).unwrap(),
        choices
    );
}
