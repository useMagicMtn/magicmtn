<h1 align="center">MagicMtn—Distributed compute for agents</h1>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn logo" width="180" height="180">
  </picture>
</p>

Distributed inference and sandboxed compute platform for agents. The open-source edition of MagicMtn Cloud that runs on your own infrastructure.

- **Inference** — Run agent workloads with models from providers such as Amazon Bedrock, SageMaker, and Ollama.
- **Sandboxed compute** — Run agent commands in isolated environments, from Apple Container on a Mac to microVMs on Linux or AWS.
- **CLI** — Use `magic` to manage sandboxes, execute commands, and install providers.
- **Provider SDK** — Build and package your own inference or compute providers in Rust.
- **Agent integrations** — Connect Pi and OpenCode to MagicMtn workspaces, or expose sandbox tools to other MCP clients.

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

Use `magic login` to connect your CLI to a hosted account. Then install the integration for your agent with `magic agents install pi` or `magic agents install opencode`. For OpenCode, use version 2.3.2 or later. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE). Third-party components retain their own licenses.
