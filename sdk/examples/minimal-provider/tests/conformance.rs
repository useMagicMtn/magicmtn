use magicmtn_provider_framework::host::{Client, Package, Registry};
use magicmtn_sdk::{
    inference::ModelConfiguration,
    plugin::{Operation, Value, PROTOCOL_VERSION},
    InferenceRequest,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
fn executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_minimal-magicmtn-provider"))
}
fn infer() -> Operation {
    Operation::Infer {
        request: InferenceRequest {
            model: ModelConfiguration {
                model: "echo".into(),
                provider: Some("minimal".into()),
                version: None,
                parameters: json!({}),
            },
            task: "echo".into(),
            input: json!({"message":"hello"}),
        },
    }
}
fn package(checksum: bool, platform: bool) -> Vec<u8> {
    let exe = std::fs::read(executable()).unwrap();
    let package = Package {
        name: "minimal".into(),
        version: "0.1.0".into(),
        protocol_version: PROTOCOL_VERSION,
        os: if platform {
            std::env::consts::OS.into()
        } else {
            "wrong".into()
        },
        arch: std::env::consts::ARCH.into(),
        sha256: if checksum {
            format!("{:x}", Sha256::digest(&exe))
        } else {
            "wrong".into()
        },
    };
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for (name, bytes) in [
        ("package.json", serde_json::to_vec(&package).unwrap()),
        ("provider", exe),
        ("LICENSE", b"Test fixture".to_vec()),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        archive.append_data(&mut h, name, bytes.as_slice()).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}
#[tokio::test]
async fn persistent_requests_and_shutdown() {
    let client = Client::start(executable(), "minimal").await.unwrap();
    let mut pid = None;
    for _ in 0..3 {
        let Value::Inference(reply) = client.call(infer(), Duration::from_secs(2)).await.unwrap()
        else {
            panic!("wrong response")
        };
        assert_eq!(reply.output["value"], json!({"message":"hello"}));
        if let Some(ref pid) = pid {
            assert_eq!(&reply.output["pid"], pid)
        } else {
            pid = Some(reply.output["pid"].clone())
        }
    }
    assert!(client
        .call(
            Operation::SandboxCreate {
                template: "x".into(),
                ttl_seconds: None,
            },
            Duration::from_secs(1)
        )
        .await
        .is_err());
    client.shutdown().await;
    assert!(client.call(infer(), Duration::from_secs(1)).await.is_err());
}
#[tokio::test]
async fn validates_packages_and_protects_live_versions() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::new(root.path().into()).unwrap();
    assert!(registry.install(&package(false, true)).await.is_err());
    assert!(registry.install(&package(true, false)).await.is_err());
    let installed = registry.install(&package(true, true)).await.unwrap();
    assert!(installed.active);
    assert_eq!(registry.list().unwrap().len(), 1);
    registry.install(&package(true, true)).await.unwrap(); // exact replay is safe
    registry
        .call(&installed.pin(), "session", infer(), Duration::from_secs(2))
        .await
        .unwrap();
    assert!(registry.remove("minimal", "0.1.0").await.is_err());
    registry.release("session").await;
    registry.remove("minimal", "0.1.0").await.unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn ambiguous_timeout_kills_connection_without_replay() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("provider");
    let manifest = magicmtn_sdk::plugin::ProviderManifest {
        credential: None,
        manifest_version: 1,
        provider_version: "0.1.0".into(),
        capabilities: magicmtn_sdk::plugin::Capabilities::inference(vec!["echo".into()]),
    };
    let reply = magicmtn_sdk::plugin::Response {
        id: 1,
        protocol_version: PROTOCOL_VERSION,
        provider: "minimal".into(),
        outcome: magicmtn_sdk::plugin::Outcome::Ok(Value::Description(manifest)),
    };
    let reply = serde_json::to_string(&reply).unwrap();
    std::fs::write(
        &path,
        format!("#!/bin/sh\nread frame\nprintf '%s\\n' '{reply}'\nread frame\nexec sleep 30\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let client = Client::start(&path, "minimal").await.unwrap();
    assert!(client
        .call(infer(), Duration::from_millis(30))
        .await
        .is_err());
    let error = client
        .call(infer(), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not replayed"));
}
