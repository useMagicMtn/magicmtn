//! The single-user deployment's image catalog and immutable provider preparations.
use crate::{provider_connections::Binding, sandbox_storage::sqlite::Database};
use anyhow::{ensure, Context, Result};
use magicmtn_sdk::image_registry::{canonical, Image, PreparedImage};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub type Manifests = BTreeMap<String, Image>;

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_".contains(&c))
}

pub async fn seed(db: &Database) -> Result<()> {
    let manifests: Manifests = ["linux/arm64", "linux/amd64"]
        .into_iter()
        .map(|platform| Ok((platform.to_owned(), canonical(platform)?)))
        .collect::<Result<_>>()?;
    sqlx::query("INSERT INTO sandbox_images(name,source,manifests) VALUES('canonical','bundled',?) ON CONFLICT(name) DO NOTHING")
        .bind(json!(manifests)).execute(&db.pool).await?;
    Ok(())
}

/// Upgrade the default once at server startup. Catalog reads never rebind it.
pub async fn refresh_canonical(db: &Database) -> Result<()> {
    seed(db).await?;
    let images: Manifests = ["linux/arm64", "linux/amd64"]
        .into_iter()
        .map(|platform| Ok((platform.to_owned(), canonical(platform)?)))
        .collect::<Result<_>>()?;
    sqlx::query("UPDATE sandbox_images SET manifests=? WHERE name='canonical'")
        .bind(json!(images))
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn manifests(db: &Database, name: &str) -> Result<Manifests> {
    let value: Value = sqlx::query_scalar("SELECT manifests FROM sandbox_images WHERE name=?")
        .bind(name)
        .fetch_optional(&db.pool)
        .await?
        .context("unknown image; use `magic --local images list`")?;
    Ok(serde_json::from_value(value)?)
}

pub async fn add(db: &Database, name: &str, source: &str, manifests: &Manifests) -> Result<()> {
    ensure!(valid_name(name) && name != "canonical", "choose an image name using lowercase letters, digits, hyphens, or underscores; canonical is reserved");
    ensure!(
        !manifests.is_empty(),
        "image has no supported Linux platform"
    );
    sqlx::query("INSERT INTO sandbox_images(name,source,manifests) VALUES(?,?,?) ON CONFLICT(name) DO NOTHING")
        .bind(name).bind(source).bind(json!(manifests)).execute(&db.pool).await?;
    ensure!(
        self::manifests(db, name).await? == *manifests,
        "this image name already identifies another digest; choose a new name"
    );
    Ok(())
}

pub async fn prepared(
    db: &Database,
    name: &str,
    binding: &Binding,
) -> Result<Option<PreparedImage>> {
    let current = manifests(db, name).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT result FROM image_preparations WHERE name=? AND connection_id=? AND provider_sha256=? AND generation=?")
        .bind(name).bind(binding.connection_id.to_string()).bind(&binding.pin.sha256).bind(binding.generation).fetch_all(&db.pool).await?;
    for row in rows {
        let result: PreparedImage = serde_json::from_value(row)?;
        if current.get(&result.platform) == Some(&result.image) {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

pub async fn save(
    db: &Database,
    name: &str,
    binding: &Binding,
    result: &PreparedImage,
) -> Result<()> {
    ensure!(
        manifests(db, name).await?.get(&result.platform) == Some(&result.image),
        "provider prepared a different image or platform"
    );
    ensure!(
        !result.artifact.is_empty() && result.artifact.len() <= 512,
        "invalid prepared artifact"
    );
    for (key, value) in &result.configuration {
        let image_mapping = matches!(
            key.as_str(),
            "MAGICMTN_MODAL_PREPARED_IMAGES"
                | "MAGICMTN_E2B_TEMPLATES"
                | "MAGICMTN_FIRECRACKER_IMAGES"
                | "MAGICMTN_DAYTONA_SNAPSHOTS"
                | "MAGICMTN_BOAT_SNAPSHOTS"
                | "MAGICMTN_TOGETHER_SNAPSHOTS"
                | "MAGICMTN_AWS_MICROVM_IMAGES"
        );
        ensure!(
            image_mapping
                && crate::provider_connections::configuration_keys(&binding.pin.name)
                    .contains(&key.as_str())
                && value.len() <= 65536,
            "provider returned an unsupported image configuration key"
        );
        let mapping: BTreeMap<String, String> =
            serde_json::from_str(value).context("invalid provider image mapping")?;
        ensure!(
            mapping.len() == 1 && mapping.get(&result.image.reference()) == Some(&result.artifact),
            "provider returned a different image artifact mapping"
        );
    }
    sqlx::query("INSERT INTO image_preparations(name,connection_id,provider_sha256,generation,platform,repository,digest,result) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT DO UPDATE SET result=excluded.result")
        .bind(name).bind(binding.connection_id.to_string()).bind(&binding.pin.sha256).bind(binding.generation)
        .bind(&result.platform).bind(&result.image.repository).bind(&result.image.digest).bind(json!(result)).execute(&db.pool).await?;
    Ok(())
}

pub async fn list(db: &Database) -> Result<Value> {
    seed(db).await?;
    let rows: Vec<(String, String, Value)> = sqlx::query_as(
        "SELECT name,source,manifests FROM sandbox_images ORDER BY name<>'canonical',name",
    )
    .fetch_all(&db.pool)
    .await?;
    let bindings: Vec<Value> =
        sqlx::query_scalar("SELECT binding FROM provider_connections WHERE status='enabled'")
            .fetch_all(&db.pool)
            .await?;
    let mut images = Vec::new();
    for (name, source, manifests) in rows {
        let mut preparations = Vec::new();
        for value in &bindings {
            let binding: Binding = serde_json::from_value(value.clone())?;
            if binding.manifest.capabilities.sandbox_operations.is_empty() {
                continue;
            }
            let result = prepared(db, &name, &binding).await?;
            preparations.push(json!({"provider":binding.pin.name,"connection_id":binding.connection_id,"ready":result.is_some(),"platform":result.as_ref().map(|r|&r.platform),"artifact":result.as_ref().map(|r|&r.artifact)}));
        }
        images.push(json!({"name":name,"source":source,"is_default":name=="canonical","manifests":manifests,"preparations":preparations}));
    }
    Ok(json!({"images":images}))
}

/// Resolve catalog names only. Explicit raw OCI references keep their existing semantics.
pub async fn resolve_launch(
    db: &Database,
    binding: &mut Binding,
    template: &mut String,
    saved: Option<&Value>,
) -> Result<Option<String>> {
    let name = if template.is_empty() {
        "canonical"
    } else {
        template.strip_prefix("image:").unwrap_or(template)
    }
    .to_owned();
    let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM sandbox_images WHERE name=?")
        .bind(&name)
        .fetch_one(&db.pool)
        .await?;
    if exists == 0 {
        return Ok(None);
    }
    if let Some(saved) =
        saved.filter(|record| record["metadata"]["_magicmtn_image_selection"] == name)
    {
        *template = saved["template"]
            .as_str()
            .context("saved image identity missing")?
            .to_owned();
        return Ok(Some(name));
    }
    let result = prepared(db, &name, binding).await?.with_context(|| format!("image {name} is not prepared for {}; run `magic --local images prepare {name} --provider {}`", binding.pin.name, binding.pin.name))?;
    *template = format!("image:{}", result.image.reference());
    binding.configuration.extend(result.configuration);
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use magicmtn_provider_framework::host::Pin;
    use magicmtn_sdk::plugin::{Capabilities, ProviderManifest, SandboxOperation};

    async fn setup() -> (tempfile::TempDir, Database, Binding) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("local.sqlite"))
            .await
            .unwrap();
        seed(&db).await.unwrap();
        let binding = Binding {
            connection_id: uuid::Uuid::new_v4(),
            organization_id: db.id,
            generation: 1,
            connection_ref: "connection:self-modal".into(),
            pin: Pin {
                name: "modal".into(),
                version: "1.0.0".into(),
                sha256: "a".repeat(64),
            },
            manifest: ProviderManifest {
                manifest_version: 1,
                provider_version: "1.0.0".into(),
                credential: None,
                capabilities: Capabilities::sandbox(vec![
                    SandboxOperation::PrepareImage,
                    SandboxOperation::Create,
                ]),
            },
            configuration: Default::default(),
            credential_mode: "managed".into(),
            credential_ref: Some("local:modal".into()),
            runner_id: None,
        };
        sqlx::query(
            "INSERT INTO provider_connections(id,reference,provider,binding) VALUES(?,?,?,?)",
        )
        .bind(binding.connection_id.to_string())
        .bind(&binding.connection_ref)
        .bind("modal")
        .bind(json!(binding))
        .execute(&db.pool)
        .await
        .unwrap();
        (dir, db, binding)
    }

    fn preparation() -> PreparedImage {
        let image = canonical("linux/amd64").unwrap();
        PreparedImage {
            configuration: BTreeMap::from([(
                "MAGICMTN_MODAL_PREPARED_IMAGES".into(),
                json!({image.reference():"im-test"}).to_string(),
            )]),
            image,
            platform: "linux/amd64".into(),
            artifact: "im-test".into(),
        }
    }

    #[tokio::test]
    async fn catalog_seeds_canonical_and_only_marks_successful_preparations_ready() {
        let (_dir, db, binding) = setup().await;
        let before = list(&db).await.unwrap();
        assert_eq!(before["images"][0]["name"], "canonical");
        assert_eq!(before["images"][0]["preparations"][0]["ready"], false);
        save(&db, "canonical", &binding, &preparation())
            .await
            .unwrap();
        let after = list(&db).await.unwrap();
        assert_eq!(after["images"][0]["preparations"][0]["ready"], true);
    }

    #[tokio::test]
    async fn registration_is_immutable_and_canonical_is_reserved() {
        let (_dir, db, _) = setup().await;
        let images = Manifests::from([("linux/amd64".into(), canonical("linux/amd64").unwrap())]);
        add(&db, "my-dev", "example/dev:1", &images).await.unwrap();
        add(&db, "my-dev", "example/dev:1", &images).await.unwrap();
        let mut changed = images.clone();
        changed.get_mut("linux/amd64").unwrap().digest = "sha256:".to_owned() + &"b".repeat(64);
        assert!(add(&db, "my-dev", "example/dev:2", &changed).await.is_err());
        assert!(add(&db, "canonical", "example/dev:1", &images)
            .await
            .is_err());
        assert!(add(&db, "../escape", "example/dev:1", &images)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn preparation_is_scoped_to_connection_generation_release_and_source() {
        let (_dir, db, mut binding) = setup().await;
        save(&db, "canonical", &binding, &preparation())
            .await
            .unwrap();
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_some());
        binding.generation += 1;
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_none());
        binding.generation -= 1;
        binding.pin.sha256 = "c".repeat(64);
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_none());
        binding.pin.sha256 = "a".repeat(64);
        binding.connection_id = uuid::Uuid::new_v4();
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn launch_uses_provider_platform_and_preserves_a_replayed_image_after_upgrade() {
        let (_dir, db, mut binding) = setup().await;
        let mut template = String::new();
        assert!(resolve_launch(&db, &mut binding, &mut template, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("images prepare"));
        let image = preparation();
        save(&db, "canonical", &binding, &image).await.unwrap();
        resolve_launch(&db, &mut binding, &mut template, None)
            .await
            .unwrap();
        assert_eq!(template, format!("image:{}", image.image.reference()));
        assert!(binding.configuration["MAGICMTN_MODAL_PREPARED_IMAGES"].contains("im-test"));
        let saved =
            json!({"template":template,"metadata":{"_magicmtn_image_selection":"canonical"}});
        let mut changed = manifests(&db, "canonical").await.unwrap();
        changed.get_mut("linux/amd64").unwrap().digest = "sha256:".to_owned() + &"c".repeat(64);
        sqlx::query("UPDATE sandbox_images SET manifests=? WHERE name='canonical'")
            .bind(json!(changed))
            .execute(&db.pool)
            .await
            .unwrap();
        template = "image:canonical".into();
        resolve_launch(&db, &mut binding, &mut template, Some(&saved))
            .await
            .unwrap();
        assert_eq!(template, saved["template"]);
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn mismatched_provider_result_is_never_saved() {
        let (_dir, db, binding) = setup().await;
        let mut image = preparation();
        image.image.digest = "sha256:".to_owned() + &"d".repeat(64);
        assert!(save(&db, "canonical", &binding, &image).await.is_err());
        assert!(prepared(&db, "canonical", &binding)
            .await
            .unwrap()
            .is_none());
    }
}
