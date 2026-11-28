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

The shell installer counts completed installs. To [opt out](documentation/docs/security.md#installation-count), run `curl -fsSL https://magicmtn.ai/install.sh | MAGIC_NO_INSTALL_ANALYTICS=1 sh`.

On macOS, you can also use Homebrew.

```sh
brew install useMagicMtn/tap/magic
```

See the [CLI installation guide](https://magicmtn.ai/docs/cli-basics/#install) for more options.

### Run your first local sandbox

Install [Apple Container](https://github.com/apple/container#get-started).

Start Apple Container and install the MagicMtn provider:

```sh
container system start
magic provider install apple-container
```

If you previously signed in to MagicMtn Cloud, run `magic logout`. Start the local server in its own terminal and keep it open:

```sh
magic server start
```

In another terminal, prepare the bundled Canonical image, create a sandbox, run a command, and delete the sandbox:

```sh
magic images prepare canonical --provider apple-container
sandbox_id=$(magic computer new --image canonical --provider apple-container)
magic computer exec "$sandbox_id" -- /bin/echo "hello from the sandbox"
magic computer terminate "$sandbox_id" --yes
```

The command result includes `hello from the sandbox`. Run `magic server stop` when finished. See the [local server guide](docs/local-server.md) for custom images, configuration, and cleanup.

### Add inference with Modal and Pi

Use Modal for inference while Pi works inside a MagicMtn Canonical [^1] VM on your Apple silicon Mac. MagicMtn manages Pi, the inference connection, and the sandbox lifecycle.

#### Install Modal and set up access

Install the [Modal provider](https://magicmtn.ai/registry/modal) and the Apple Container provider from the MagicMtn Registry:

```sh
magic provider install modal
magic provider install apple-container
```

Create a [Modal workspace API token](https://modal.com/docs/guide/workspaces) with access to manage endpoints and Proxy Tokens. Save it as `modal-credentials.json`:

```json
{
  "MODAL_TOKEN_ID": "YOUR_TOKEN_ID",
  "MODAL_TOKEN_SECRET": "YOUR_TOKEN_SECRET"
}
```

Start the local server in its own terminal and keep it open:

```sh
magic server start
```

In another terminal, configure Modal:

```sh
magic provider configure modal --secret-file ./modal-credentials.json
```

This establishes access to your Modal account. Model selection determines which inference resource MagicMtn provisions; the provider manages its endpoint and credentials.

#### Prepare local isolation with Apple Container

Install [Apple Container](https://github.com/apple/container#get-started) on your Apple silicon Mac, start its runtime, and make the Canonical image available to it:

```sh
container system start
magic images prepare canonical --provider apple-container
```

#### Run MagicMtn-managed Pi

Install [Pi](documentation/docs/coding-agents/pi.md#install-pi), then install the MagicMtn Pi integration and open Pi:

```sh
magic agents install pi
pi
```

In Pi, run `/magic default` and select **Kimi K3** with **Modal** as the inference provider. Run `/magic new` and choose **Apple Container** as the sandbox backend.

MagicMtn starts Pi inside the local VM and routes its model requests to your Modal endpoint.

Example: Ask Pi to work in its isolated workspace:

> Create a Python script in /computer that prints the first 20 Fibonacci numbers, run it, and show me the output.

Press **Ctrl+]** to disconnect while Pi keeps running. Use `/magic view` and choose **Resume** to reconnect, or **Terminate** to delete the workspace when finished.

#### Bonus: isolation with Modal sandboxes

Run the same Pi workflow inside a sandbox in your own Modal account.

Reuse the configured Modal connection and selected model, and make the Canonical image available to Modal:

```sh
magic images prepare canonical --provider modal
pi
```

Keep the same model in `/magic default`. Run `/magic new` and choose **Modal** as the sandbox backend. MagicMtn runs Pi in the Modal sandbox and sends inference requests to the same model endpoint.

Use the same prompt, **Ctrl+]** to disconnect, and `/magic view` to resume or terminate the workspace. After terminating your workspaces, run `magic server stop` to stop the local server.

See the [Modal provider setup guide](https://github.com/useMagicMtn/provider-modal/blob/main/USAGE.md) for account and endpoint configuration.

## Use the MagicMtn Web UI

Start the server with `magic server start`, then open <http://localhost:1900> to view your computers and images, configure templates, and browse the MCP Catalog. Sign in as `magic` with the dashboard password printed in the terminal.

See the [Web UI guide](https://magicmtn.ai/docs/web-ui/) for a tour.

## Using MagicMtn Cloud

Sign up for [MagicMtn Cloud](https://magicmtn.ai/), our managed platform for inference and cloud computers for agents.

### Set up Pi

Install the its Pi extension, and open Pi.

```sh
magic login
magic agents install pi
pi
```

In Pi, run `/magic new` to start a managed workspace. Press **Ctrl+]** to disconnect while Pi keeps running, then use `/magic view` and choose **Resume** to rejoin. [Pi setup guide](https://magicmtn.ai/docs/coding-agents/pi/)

For OpenCode 2.3.2 or later, sign in with `magic login`, then run `magic agents install opencode`. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Install from source

Install Rust 1.100.0 or later and a C/C++ build toolchain (Xcode Command Line Tools on macOS, or GCC and Make on Linux). Download and extract the [source archive](https://github.com/useMagicMtn/magicmtn/archive/refs/heads/main.tar.gz), then run these commands from the extracted repository root:

```sh
cargo install --path cli --locked --bin magic
magic --version
```

Cargo installs `magic` in `~/.cargo/bin`; add that directory to your PATH. To build a provider in a separate project, start with the [provider SDK example](sdk/examples/minimal-provider/README.md).

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Read the [security model](documentation/docs/security.md) for sandbox boundaries and provider trust. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE).

[^1]: MagicMtn Canonical is the default machine/sandbox image.
