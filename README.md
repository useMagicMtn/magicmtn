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

See the [CLI installation guide](https://magicmtn.ai/docs/cli-basics/#install) for more options.

### Set up Pi

Sign in to MagicMtn Cloud, install the MagicMtn extension, and open Pi.

```sh
magic login
magic agents install pi
pi
```

In Pi, run `/magic new` to start a managed workspace. Press **Ctrl+]** to disconnect while Pi keeps running, then use `/magic view` and choose **Resume** to rejoin. [Pi setup guide](https://magicmtn.ai/docs/coding-agents/pi/)

### Example A: Run a local sandbox provider (e.g. Apple Container)

Run a sandbox on your Apple silicon Mac, execute a task inside it, and connect Pi to local sandboxes. Install [Apple Container](https://github.com/apple/container), then start its runtime and install the MagicMtn provider.

```sh
container system start
magic provider install apple-container
```

Start the local server in its own terminal with internet access for new sandboxes.

```sh
magic server start
```

Keep this terminal open. In another terminal, prepare the bundled Canonical image and create a sandbox.

```sh
magic images list
magic images prepare canonical --provider apple-container
sandbox_id=$(magic computer new --image canonical --provider apple-container)
magic computer list
```

Run a Python task that writes a file inside the sandbox, then read the file in a separate command.

```sh
magic computer exec "$sandbox_id" -- python3 -c 'from pathlib import Path; Path("/workspace/hello.txt").write_text("Hello from MagicMtn!\n")'
magic computer exec "$sandbox_id" -- cat /workspace/hello.txt
```

The second command returns `Hello from MagicMtn!` in its output. Delete the sandbox and its files when finished.

```sh
magic computer terminate "$sandbox_id" --yes
```

To let Pi create and work in local sandboxes, install the extension and start Pi.

```sh
magic agents install pi
pi
```

In Pi, use `/login` to connect your inference provider and `/model` to choose a model. Run `/magic new` and select Canonical with Apple Container, then ask Pi to create and run a Python script in the sandbox. Use `/magic view` to manage your sandboxes.

After leaving Pi, run `magic computer list` and terminate any sandboxes you have finished using with `magic computer terminate <ID> --yes`. Run `magic server stop` to stop the local server. See the [local quickstart](docs/local-server.md) for custom images and configuration.

### Example B: use a cloud-based sandbox provider (e.g. Modal)

This example shows how you can run MagicMtn pointing to your own sandbox/VMs provider — in this case using Modal. MagicMtn manages the machines.

To run sandboxes in your own Modal account, install the provider from the MagicMtn Registry. [Modal provider](https://magicmtn.ai/registry/modal)

```sh
magic provider install modal
```

Installation includes the provider's private Python runtime and Modal SDK. Configuring the provider creates or selects the `magicmtn` app in your Modal account.

Start the local server in its own terminal.

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

## Use the MagicMtn Web UI

Start the server with `magic server start`, then open <http://localhost:1900> to view your computers and images, configure templates, and browse the MCP Catalog. Sign in as `magic` with the dashboard password printed in the terminal.

See the [Web UI guide](https://magicmtn.ai/docs/web-ui/) for a tour.

## Connect to hosted MagicMtn

For OpenCode 2.3.2 or later, sign in with `magic login`, then run `magic agents install opencode`. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE).
