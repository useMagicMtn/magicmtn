# Release MagicMtn Canonical

Canonical is built from `images/canonical/Containerfile` and published to the public OCI repository `ghcr.io/usemagicmtn/canonical`. It includes the sandbox runtime and pinned Pi, Codex, and OpenCode installations.

## Release with the CLI

Run **Publish Canonical image** on `main` when updating the image. It builds and smoke-tests Linux ARM64 and AMD64 images on native runners, publishes them to GHCR, verifies anonymous access, and produces a `canonical-image-lock` artifact. Commit that artifact as `sdk/rust/magicmtn-sdk/src/canonical-image.json`.

Run **Publish CLI binaries** on `main` to release the CLI. It verifies anonymous access to both committed image digests before building. Every CLI binary embeds the same committed lock, keeping release source clean and image selection consistent with Cloud’s pinned source revision.

The lock is `sdk/rust/magicmtn-sdk/src/canonical-image.json`. It maps OCI platforms to immutable manifest digests. The container runtime selects and stores the host's image locally. Provider installation pulls the pinned ARM64 image into Apple Container. Local CLI and MCP creation use the server's default, which is frozen in the sandbox record before the provider launches it. Replaying an existing creation intent retains its saved digest.

## GHCR access

The workflow uses its repository's `GITHUB_TOKEN` with `packages: write`. Set the `canonical` container package under the UseMagicMtn organization to **Public** in GitHub's package settings. Publication verifies access without publisher credentials before emitting the lock; the CLI release depends on successful publication.

Release tags contain the source commit, workflow run ID, and attempt. A multi-platform tag identifies both native builds. CLI binaries launch the pinned platform manifests directly.

## Publish from a release machine

Authenticate Podman to GHCR with a publisher credential that has `write:packages`. Build each architecture on a native host:

```sh
python3 scripts/build-canonical-image.py \
  --platform linux/arm64 --tag localhost/magicmtn/canonical:release-arm64 \
  --output dist/canonical
```

Use `linux/amd64` and `release-amd64` on the AMD64 host. Collect both OCI archives and metadata files into `dist/canonical`, then publish:

```sh
python3 scripts/publish-canonical-image.py \
  dist/canonical/canonical-arm64.oci.json \
  dist/canonical/canonical-amd64.oci.json \
  --tag release-VERSION
```

The publisher validates the archives and source hashes before uploading. It writes the lock atomically after both platform images and their index pass anonymous manifest verification. Commit the resulting lock to select that image release for development, CI, and production CLI builds.

## Custom images and providers

`magic computer new --image REFERENCE` passes the user's selection to the provider. Explicit selections bypass Canonical setup. Apple Container owns registry authentication, downloads, and storage for custom OCI images.

Firecracker owns prepared VM root filesystems and its image mapping through `MAGICMTN_FIRECRACKER_IMAGES`; map the Canonical reference to a prepared VM image to select it on that provider. `magic --local images list` shows the local provider's inventory. `magic images list` shows enabled images in your Cloud workspace.


## Review bundled software

Record the licenses and notices for the tools in the image before distribution. The image includes the MagicMtn license at `/usr/share/licenses/magicmtn/LICENSE`. Review the terms for preinstalled third-party agents and keep their authentication flows intact. [Claude Code preinstallation terms](https://code.claude.com/docs/en/legal-and-compliance#can-customers-offer-claude-code-in-their-products)
