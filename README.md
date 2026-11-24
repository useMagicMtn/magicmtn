<h1 align="center">MagicMtn—Distributed compute for agents</h1>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn logo" width="180" height="180">
  </picture>
</p>

<p align="center"><a href="https://magicmtn.ai/docs/">MagicMtn Docs</a> &nbsp;•&nbsp; <a href="https://magicmtn.ai/registry">Provider Registry</a> &nbsp;•&nbsp; <a href="CONTRIBUTING.md">Contributing</a> &nbsp;•&nbsp; <a href="https://magicmtn.ai/">Cloud service</a></p>

Distributed inference and sandboxed compute platform for agents. The open-source edition of MagicMtn Cloud that runs on your own infrastructure.

- **Inference** — Run agent workloads with models from providers such as Amazon Bedrock, SageMaker, and Ollama.
- **Sandboxed compute** — Run agent commands in isolated environments, from Apple Container on a Mac to microVMs on Linux or AWS.
- **CLI** — Use `magic` to manage sandboxes, execute commands, and install providers.
- **Provider SDK** — Build and package your own inference or compute providers in Rust.
- **Agent integrations** — Connect [Pi](https://pi.dev/) and OpenCode to MagicMtn workspaces, or expose sandbox tools to other MCP clients.

## Getting Started

### Install the MagicMtn CLI

Run the installer on macOS or Linux.

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

On macOS, you can also use Homebrew.

```sh
brew install UseMagicMtn/tap/magic
```

See [Install from source](#install-from-source) and [Install with Nix](#install-with-nix) for more options.

### Set up Pi

Sign in to MagicMtn Cloud, install the MagicMtn extension, and open Pi.

```sh
magic login
magic agents install pi
pi
```

In Pi, run `/magic new` to start a managed workspace. Press **Ctrl+]** to disconnect while Pi keeps running, then use `/magic view` and choose **Resume** to rejoin. [Pi setup guide](https://magicmtn.ai/docs/coding-agents/pi/)

### Use Modal as a sandbox provider

To run sandboxes in your own Modal account, install the provider from the MagicMtn Registry. [Modal provider](https://magicmtn.ai/registry/modal)

```sh
magic provider install modal
```

Install the Modal SDK for the `python3` used by the server's provider processes, create a Modal app named `magicmtn`, and prepare a sandbox image. [Python setup](https://github.com/useMagicMtn/provider-modal/blob/main/USAGE.md#install-the-modal-sdk) · [Image preparation](https://github.com/useMagicMtn/provider-modal/blob/main/USAGE.md#prepare-a-sandbox-image)

When switching from MagicMtn Cloud, run `magic logout`. Start the local server in its own terminal with a mapping from the exact OCI image digest to its prepared Modal image ID.

```sh
MAGICMTN_MODAL_PREPARED_IMAGES='{"registry.example/sandbox@sha256:YOUR_DIGEST":"im-YOUR_IMAGE_ID"}' \
  magic server start
```

Create an API token in your Modal workspace and save it in a private file named `modal-credentials.json`. [Modal API tokens](https://modal.com/docs/guide/workspaces)

```json
{
  "MODAL_TOKEN_ID": "YOUR_TOKEN_ID",
  "MODAL_TOKEN_SECRET": "YOUR_TOKEN_SECRET"
}
```

In another terminal, use Credential Protocol v2 to verify and store the token in MagicMtn's encrypted credential store. Then start Pi.

```sh
magic --local provider configure modal --secret-file ./modal-credentials.json
magic agents install pi
pi
```

In Pi, use `/login` to connect your inference provider and `/model` to choose a model. Ask Pi to use `magic_sandbox_create` with provider `modal` and the same digest-pinned image reference, then run `pwd` using `magic_sandbox_exec`.

## Demo

https://github.com/user-attachments/assets/e42832f4-cd24-4d25-9cae-eacd053c44f1

To replay the [terminal recording](cli-pi-demo.cast), run this from the repository root.

```sh
asciinema play cli-pi-demo.cast
```

## Install from source

Use Rust 1.100.0 or a later stable release. From the repository root, install the CLI and check its version.

```sh
cargo install --path cli --locked --bin magic
magic --version
```

Cargo installs the executable into `~/.cargo/bin` by default. Add that directory to your PATH if your shell cannot find `magic`.

## Install with Nix

Run the CLI directly or install it into your profile using Nix flakes.

```sh
nix run github:useMagicMtn/magicmtn -- --version
nix profile install github:useMagicMtn/magicmtn
```

To add `magic` to a NixOS or Home Manager flake configuration, add `github:useMagicMtn/magicmtn` to your flake inputs and reference `inputs.magicmtn.packages.${pkgs.stdenv.hostPlatform.system}.default`, or apply `inputs.magicmtn.overlays.default` and install `pkgs.magicmtn`.

## Run a local sandbox

On Apple silicon, use Apple Container and the built-in SQLite server to create, execute a command in, and terminate a sandbox. Follow the [local quickstart](docs/local-server.md).

The local server also exposes sandbox tools to Pi and OpenCode. With the local server running, use `magic agents install pi` or `magic agents install opencode`. Start your agent and use `/magic new` and `/magic view` to work with sandboxes.

## Connect to hosted MagicMtn

For OpenCode 2.3.2 or later, sign in with `magic login`, then run `magic agents install opencode`. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE). Third-party components retain their own licenses.
