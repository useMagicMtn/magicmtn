//! Provider-neutral image storage for MagicMtn.
//!
//! A configured backend scopes its repository inventory and owns credentials.
//! The caller authorizes user selections, freezes resolved images in harness
//! revisions, and owns staging directories. Building and sandbox execution are
//! separate compute operations. These traits do not change the executable wire
//! protocol or automatically integrate registry retrieval with session startup.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::BoxFuture;

/// An immutable image identity, suitable for freezing in a harness revision.
///
/// `repository` is a non-secret logical name or registry/repository locator.
/// `digest` identifies the image manifest or multi-platform index, never a tag.
/// Implementations validate both fields and reject repositories outside their
/// configured scope. Possession of this value does not grant access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub repository: String,
    pub digest: String,
}

impl Image {
    /// A runnable OCI reference that never follows a mutable tag.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.repository, self.digest)
    }
}

/// A provider-owned artifact prepared from one immutable OCI platform image.
/// Configuration contains non-secret launch settings, never registry credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImage {
    pub image: Image,
    pub platform: String,
    pub artifact: String,
    #[serde(default)]
    pub configuration: std::collections::BTreeMap<String, String>,
}

/// The Canonical image release bundled with this version of MagicMtn.
/// CLI releases embed a new lock only after verifying anonymous registry pulls.
pub fn canonical(platform: &str) -> anyhow::Result<Image> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Release {
        repository: String,
        manifests: std::collections::BTreeMap<String, String>,
    }
    let release: Release = serde_json::from_str(include_str!("canonical-image.json"))?;
    let digest = release
        .manifests
        .get(platform)
        .ok_or_else(|| anyhow::anyhow!("MagicMtn Canonical has no image for {platform}"))?;
    anyhow::ensure!(
        digest.strip_prefix("sha256:").is_some_and(
            |hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        "invalid Canonical image digest"
    );
    Ok(Image {
        repository: release.repository,
        digest: digest.clone(),
    })
}

/// OCI platform used by sandbox images on this host.
pub fn local_platform() -> anyhow::Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("linux/arm64"),
        "x86_64" => Ok("linux/amd64"),
        _ => anyhow::bail!("unsupported sandbox image architecture"),
    }
}

/// Store and retrieve built container images through a provider-owned registry.
pub trait ImageRegistryBackend: Send + Sync {
    /// List selectable repository/tag or repository/digest references.
    /// Provider pagination is handled internally; ordering is provider-defined.
    fn list<'a>(&'a self) -> BoxFuture<'a, Vec<String>>;

    /// Resolve an explicit `repository:tag` or `repository@digest` to an image.
    /// No implicit `latest`; invalid or missing references return an error.
    fn resolve<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Image>;

    /// Retrieve the exact image as a complete OCI image layout.
    ///
    /// `destination` must be absent or an empty directory with an existing
    /// parent. Success means one root descriptor and all referenced manifests,
    /// configurations, and layers are present with verified digests and sizes.
    /// Multi-platform indexes retain all their referenced platform images.
    /// No unrelated files are overwritten; failure must not expose a partial
    /// layout at the destination. The caller owns the completed directory.
    fn pull<'a>(&'a self, image: &'a Image, destination: &'a Path) -> BoxFuture<'a, ()>;

    /// Publish a complete OCI layout containing exactly one root descriptor.
    ///
    /// `reference` is an explicit tag or digest in a configured repository.
    /// Tags cannot be rebound to different contents. Publishing the same image
    /// to the same reference is idempotent; return the verified image identity.
    /// A digest reference must match the source manifest/index digest.
    fn publish<'a>(&'a self, source: &'a Path, reference: &'a str) -> BoxFuture<'a, Image>;

    /// Delete the image and all of its tag bindings, preserving other images.
    /// Missing images return an error. The caller checks harness retention and
    /// deletion permissions; the registry has no knowledge of harness revisions.
    fn delete<'a>(&'a self, image: &'a Image) -> BoxFuture<'a, ()>;
}
