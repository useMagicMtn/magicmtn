//! Cloud and local image inventories and installation of the pinned Canonical release.
use crate::ShowFormat;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub(crate) async fn list_cloud(api: &crate::Api, format: ShowFormat) -> Result<()> {
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
    let mut response = api.request(reqwest::Method::GET, "v1/images", None).await?;
    if matches!(format, ShowFormat::Json) {
        response["images"]
            .as_array_mut()
            .context("invalid Cloud image inventory")?
            .retain(|image| image["status"] == "enabled");
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
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

pub(crate) async fn list_local(directory: &Path, format: ShowFormat) -> Result<()> {
    let catalog = magicmtn_local::images::catalog(directory).await?;
    if matches!(format, ShowFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
        return Ok(());
    }
    println!(
        "{:<24} | {:<7} | {:<20} | {:<12} | status",
        "name", "default", "provider", "platform"
    );
    for image in catalog["images"]
        .as_array()
        .context("invalid local image catalog")?
    {
        let preparations = image["preparations"]
            .as_array()
            .context("invalid image preparations")?;
        if preparations.is_empty() {
            println!(
                "{:<24} | {:<7} | {:<20} | {:<12} | not prepared",
                crate::table_cell(crate::text(image, "name"), 24),
                if image["is_default"] == true {
                    "yes"
                } else {
                    ""
                },
                "",
                ""
            );
        }
        for preparation in preparations {
            println!(
                "{:<24} | {:<7} | {:<20} | {:<12} | {}",
                crate::table_cell(crate::text(image, "name"), 24),
                if image["is_default"] == true {
                    "yes"
                } else {
                    ""
                },
                crate::table_cell(crate::text(preparation, "provider"), 20),
                crate::text(preparation, "platform"),
                if preparation["ready"] == true {
                    "ready"
                } else {
                    "not prepared"
                }
            );
        }
    }
    Ok(())
}

pub(crate) async fn add(directory: &Path, name: &str, source: &str) -> Result<()> {
    magicmtn_local::images::add(directory, name, source).await?;
    println!("Registered {name}. Prepare it with `magic --local images prepare {name} --provider PROVIDER`.");
    Ok(())
}

pub(crate) async fn prepare(api: &crate::Api, name: &str, provider: &str) -> Result<()> {
    anyhow::ensure!(valid_image_name(name), "invalid image name");
    eprintln!("Preparing {name} on {provider}…");
    let response = api
        .request(
            reqwest::Method::POST,
            &format!("v1/images/{name}/prepare"),
            Some(serde_json::json!({"provider":provider})),
        )
        .await?;
    println!(
        "{name} is ready on {provider} ({})",
        crate::text(&response["prepared"], "platform")
    );
    Ok(())
}

fn valid_image_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
}

pub(crate) fn local_provider() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("apple-container"),
        "linux" => Ok("firecracker"),
        _ => bail!("local sandboxes require macOS or Linux"),
    }
}

/// Omitting the template lets the server freeze its default before dispatch.
/// Replaying the intent then retains the saved digest across default upgrades.
pub(crate) fn creation_body(
    image: Option<&str>,
    provider: Option<&str>,
    key: uuid::Uuid,
) -> Result<serde_json::Value> {
    let provider = match provider {
        Some(provider) => provider,
        None => local_provider()?,
    };
    let mut body = serde_json::json!({"provider": provider, "idempotency_key": key});
    if let Some(image) = image {
        body["template"] = serde_json::json!(format!("image:{image}"));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
