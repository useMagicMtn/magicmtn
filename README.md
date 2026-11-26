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

## Demo

https://github.com/user-attachments/assets/e42832f4-cd24-4d25-9cae-eacd053c44f1

_Launch a cloud agent from Pi, disconnect while it keeps running, then SSH into the machine MagicMtn provisioned._

## Getting Started

### Install the MagicMtn CLI

Run the installer on macOS or Linux.

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

On macOS, you can also use Homebrew.

```sh
brew install useMagicMtn/tap/magic
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

### Example: use Modal as a sandbox provider

This example shows how you can run MagicMtn pointing to your own sandbox/VMs provider — in this case using Modal. MagicMtn manages the machines.

To run sandboxes in your own Modal account, install the provider from the MagicMtn Registry. [Modal provider](https://magicmtn.ai/registry/modal)

```sh
magic provider install modal
```

Installation includes the provider's private Python runtime and Modal SDK. Configuring the provider creates or selects the `magicmtn` app in your Modal account.

When switching from MagicMtn Cloud, run `magic logout`. Start the local server in its own terminal.

```sh
magic server start
```

Create an API token in your Modal workspace and save it in a private file named `modal-credentials.json`. [Modal API tokens](https://modal.com/docs/guide/workspaces)

```json
{
  "MODAL_TOKEN_ID": "YOUR_TOKEN_ID",
  "MODAL_TOKEN_SECRET": "YOUR_TOKEN_SECRET"
}
```

In another terminal, verify and store the token, prepare the bundled Canonical image on Modal, and start Pi.

```sh
magic provider configure modal --secret-file ./modal-credentials.json
magic images list
magic images prepare canonical --provider modal
magic agents install pi
pi
```

In Pi, use `/login` to connect your inference provider and `/model` to choose a model. Run `/magic new` to select a prepared image and provider. Pi can run commands and work with files in the sandbox through the MagicMtn tools.

To use your own image, register and prepare it without editing Canonical:

```sh
magic images add my-dev --from ghcr.io/my-org/dev:1.0
magic images prepare my-dev --provider modal
```

The image must include the sandbox runtime and Pi; see [custom images](docs/local-server.md#add-a-custom-image). Preparation validates the image and saves its Modal image ID locally, so users do not maintain image mappings.

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

Open <http://localhost:1900> to view computers, images, and installed providers. Use Templates to configure GitHub repositories for new computers, and browse available MCP services in the MCP Catalog. The sandbox API listens on port 1924. The local server also exposes sandbox tools to Pi and OpenCode. With the local server running, use `magic agents install pi` or `magic agents install opencode`. Start your agent and use `/magic new` and `/magic view` to work with sandboxes.

## Connect to hosted MagicMtn

For OpenCode 2.3.2 or later, sign in with `magic login`, then run `magic agents install opencode`. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE). Third-party components retain their own licenses.
