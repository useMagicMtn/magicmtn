//! Cloud and local image inventories and installation of the pinned Canonical release.
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

pub(crate) async fn list_cloud(api: &crate::Api) -> Result<()> {
    anyhow::ensure!(api.token.is_some(), "sign in first with `magic login`");
    #[derive(serde::Deserialize)]
    struct Image {
        id: String,
        display_name: String,
        description: String,
        is_default: bool,
        status: String,
    }
    #[derive(serde::Deserialize)]
    struct Inventory {
        images: Vec<Image>,
    }
    let response = api.request(reqwest::Method::GET, "v1/images", None).await?;
    let inventory: Inventory =
        serde_json::from_value(response).context("reading Cloud image inventory")?;
    println!(
        "{:<36} | {:<32} | {:<7} | description",
        "id", "name", "default"
    );
    for image in inventory
        .images
        .into_iter()
        .filter(|image| image.status == "enabled")
    {
        println!(
            "{:<36} | {:<32} | {:<7} | {}",
            crate::table_cell(&image.id, 36),
            crate::table_cell(&image.display_name, 32),
            if image.is_default { "yes" } else { "" },
            crate::table_cell(&image.description, 80),
        );
    }
    Ok(())
}

pub(crate) fn list_local() -> Result<()> {
    match std::env::consts::OS {
        "macos" => {
            let status = std::process::Command::new("container")
                .args(["image", "list"])
                .status()
                .context("listing local images with Apple Container")?;
            anyhow::ensure!(
                status.success(),
                "Apple Container image listing failed: {status}"
            );
            Ok(())
        }
        "linux" => {
            let mapping =
                std::env::var("MAGICMTN_FIRECRACKER_IMAGES").or_else(|error| match error {
                    std::env::VarError::NotPresent => Ok("{}".to_owned()),
                    error => Err(error),
                })?;
            let images: BTreeMap<String, String> =
                serde_json::from_str(&mapping).context("reading MAGICMTN_FIRECRACKER_IMAGES")?;
            println!("{:<48} | image", "reference");
            for (reference, image) in images {
                println!(
                    "{:<48} | {}",
                    crate::table_cell(&reference, 48),
                    crate::table_cell(&image, 80)
                );
            }
            Ok(())
        }
        _ => bail!("local sandbox images require macOS or Linux"),
    }
}

pub(crate) fn local_provider() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("apple-container"),
        "linux" => Ok("firecracker"),
        _ => bail!("local sandboxes require macOS or Linux"),
    }
}

pub(crate) fn canonical_reference() -> Result<String> {
    use magicmtn_sdk::image_registry::{canonical, local_platform};
    Ok(canonical(local_platform()?)?.reference())
}

/// The runtime owns layers and verifies their content digests during pulls.
/// Inspect the exact reference on every use so deleting an image is recoverable.
pub(crate) fn ensure_canonical(provider: &str) -> Result<()> {
    if provider != "apple-container" {
        // Other providers own image preparation (for example Firecracker rootfs).
        return Ok(());
    }
    let reference = canonical_reference()?;
    let inspect = || {
        std::process::Command::new("container")
            .args(["image", "inspect", &reference])
            .output()
            .context("inspecting MagicMtn Canonical with Apple Container")
    };
    let output = inspect()?;
    if output.status.success() {
        return verify_canonical(&output.stdout);
    }
    eprintln!("Downloading MagicMtn Canonical...");
    // Reserve stdout for command results and MCP JSON-RPC messages.
    let status = std::process::Command::new("container")
        .args([
            "image",
            "pull",
            "--platform",
            magicmtn_sdk::image_registry::local_platform()?,
            &reference,
        ])
        .stdout(std::process::Stdio::from(std::io::stderr()))
        .spawn()
        .context("downloading MagicMtn Canonical with Apple Container")?
        .wait()?;
    anyhow::ensure!(
        status.success(),
        "MagicMtn Canonical download failed: {status}; retry local provider installation"
    );
    let output = inspect()?;
    anyhow::ensure!(
        output.status.success(),
        "could not inspect the downloaded Canonical image"
    );
    verify_canonical(&output.stdout)?;
    eprintln!("MagicMtn Canonical is ready.");
    Ok(())
}

fn verify_canonical(bytes: &[u8]) -> Result<()> {
    use magicmtn_sdk::image_registry::{canonical, local_platform};
    let platform = local_platform()?;
    let image = canonical(platform)?;
    let architecture = platform
        .strip_prefix("linux/")
        .context("invalid Canonical platform")?;
    let records: Vec<serde_json::Value> =
        serde_json::from_slice(bytes).context("reading Apple Container image metadata")?;
    anyhow::ensure!(
        records.iter().any(|record| {
            record["configuration"]["name"] == image.reference()
                && record["variants"].as_array().is_some_and(|variants| {
                    variants.iter().any(|variant| {
                        variant["digest"] == image.digest
                            && variant["platform"]["os"] == "linux"
                            && variant["platform"]["architecture"] == architecture
                    })
                })
        }),
        "local Canonical image does not match its pinned digest and platform"
    );
    Ok(())
}

/// Omitting the template lets the server freeze its default before dispatch.
/// Replaying the intent then retains the saved digest across default upgrades.
pub(crate) fn creation_body(
    image: Option<&str>,
    provider: Option<&str>,
    key: uuid::Uuid,
) -> Result<serde_json::Value> {
    let provider = provider.unwrap_or(local_provider()?);
    let mut body = serde_json::json!({"provider": provider, "idempotency_key": key});
    match image {
        Some(image) => body["template"] = serde_json::json!(format!("image:{image}")),
        None => ensure_canonical(provider)?,
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cached_image_requires_exact_reference_digest_and_platform() {
        let platform = magicmtn_sdk::image_registry::local_platform().unwrap();
        let image = magicmtn_sdk::image_registry::canonical(platform).unwrap();
        let mut records = json!([{
            "configuration": {"name": image.reference()},
            "variants": [{"digest": image.digest, "platform": {
                "os": "linux", "architecture": platform.split('/').nth(1).unwrap()
            }}]
        }]);
        assert!(verify_canonical(&serde_json::to_vec(&records).unwrap()).is_ok());
        for (path, invalid) in [
            ("/0/configuration/name", "custom:latest"),
            ("/0/variants/0/digest", "sha256:wrong"),
            ("/0/variants/0/platform/os", "windows"),
            ("/0/variants/0/platform/architecture", "wrong"),
        ] {
            let original = records.pointer(path).unwrap().clone();
            *records.pointer_mut(path).unwrap() = json!(invalid);
            assert!(verify_canonical(&serde_json::to_vec(&records).unwrap()).is_err());
            *records.pointer_mut(path).unwrap() = original;
        }
        assert!(verify_canonical(b"[]").is_err());
        assert!(verify_canonical(b"invalid JSON").is_err());
    }

    #[test]
    fn custom_image_does_not_access_the_runtime_or_change_the_reference() {
        let key = uuid::Uuid::new_v4();
        let body =
            creation_body(Some("localhost/custom:dev"), Some("custom-provider"), key).unwrap();
        assert_eq!(
            body,
            json!({
                "template": "image:localhost/custom:dev",
                "provider": "custom-provider", "idempotency_key": key
            })
        );
    }
}
