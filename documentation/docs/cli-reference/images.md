---
title: images
---

List images available in the selected deployment. `magic login` selects MagicMtn Cloud; `magic server start` selects your standalone deployment.

## list

```sh
magic images list
```

With MagicMtn Cloud selected, sign in with `magic login` first. The command lists enabled Cloud images with their IDs, names, default selection, and descriptions. MagicMtn Canonical follows the current Cloud release; previous releases retained for rollback do not appear as selectable images.

## List local images

```sh
magic --local images list
```

Use `--local` to select the local catalog explicitly. After `magic server start` selects your deployment, `magic images list` uses that catalog automatically. Listing works without a running server or signing in. It lists the local deployment's registered images and preparation status per provider. Canonical is included as the default; container runtime dependencies are outside this catalog. Use `--format json` for machine-readable output.

## Prepare an image

With the local server running and the provider configured:

```sh
magic images prepare canonical --provider modal
```

Preparation imports or pulls the image for the provider's platform, validates its sandbox runtime and Pi, and saves the prepared artifact locally. Repeated calls reuse the saved preparation. Create a sandbox with `magic computer new --image canonical --provider modal`.

## Add a custom image

```sh
magic images add my-dev --from ghcr.io/my-org/dev:1.0
magic images prepare my-dev --provider modal
magic computer new --image my-dev --provider modal
```

Registration resolves the source tag to immutable platform digests. Use a new name for another version. Custom images must be readable from their registry and include Python, Pi, the MagicMtn sandbox runtime, and a writable `/workspace` for sandbox user 10001. Preparation validates the image without modifying it or Canonical. Pi's `/magic new` picker offers prepared image and provider combinations. See the [computer command reference](computer.md) for execution options.
