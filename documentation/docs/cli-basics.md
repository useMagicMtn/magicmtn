---
title: CLI basics
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';

The MagicMtn CLI is called `magic`. Use it with [MagicMtn Cloud](./cloud/index.md) or your own [open source installation](./open-source/index.md).

## Install

<Tabs defaultValue="macos">
<TabItem value="macos" label="macOS">

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

The installer sets up `magic` in `~/.local/bin` and offers to configure your shell. Restart your terminal when it finishes.

Authenticate Git with a GitHub account that has access to the MagicMtn tap before installing with Homebrew.

```sh
brew install UseMagicMtn/tap/magic
```

</TabItem>
<TabItem value="linux" label="Linux">

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

The installer sets up `magic` in `~/.local/bin` and offers to configure your shell. Restart your terminal when it finishes.

</TabItem>
<TabItem value="nix" label="Nix">

With Nix installed and [flakes enabled](https://nix.dev/concepts/flakes.html#running-commands), install `magic` into your Nix profile and check that it’s available.

```sh
nix profile install github:useMagicMtn/magicmtn
magic --version
```

To try the CLI without installing it into your profile, run:

```sh
nix run github:useMagicMtn/magicmtn -- --version
```

To manage the CLI in a NixOS or Home Manager configuration, add `github:useMagicMtn/magicmtn` to your flake inputs. Install `inputs.magicmtn.packages.${pkgs.stdenv.hostPlatform.system}.default`, or apply `inputs.magicmtn.overlays.default` and install `pkgs.magicmtn`.

</TabItem>
<TabItem value="source" label="From source">

Use Rust 1.100.0 or a later stable release. From the MagicMtn repository root, install the CLI and check its version.

```sh
cargo install --path cli --locked --bin magic
magic --version
```

Cargo installs `magic` into `~/.cargo/bin`. Add that directory to your PATH if your shell cannot find it.

</TabItem>
<TabItem value="powershell" label="Powershell">

A native Windows binary is not available yet. In WSL with a supported Linux distribution, use the instructions in the Linux tab.

</TabItem>
</Tabs>

## Run locally

Follow the [local server setup](cli-reference/server.md) to run sandboxes through providers installed on your machine. Starting the server selects your local installation for subsequent commands. In another terminal, list its computers:

```sh
magic computer list
```

Open the [Web UI](./web-ui.md) at [http://localhost:1900](http://localhost:1900) to inspect your installation. See [server selection](cli-reference.md#select-a-server) to choose a deployment explicitly.

## Cloud login {#login}

```sh
magic login
```

The command opens your browser. Sign in to MagicMtn and approve CLI access, then return to your terminal. If the browser does not open automatically, open the URL printed in the terminal.

If you belong to an organization, the CLI uses it automatically. Otherwise, it uses your personal account.

## Cloud logout {#logout}

```sh
magic logout
```

This revokes your CLI session and removes its saved credentials from this computer.

## MCP

Connect an MCP client to your selected deployment's sandbox tools with `magic mcp serve`. Start your local server or sign in to Cloud before connecting the client. See the [MCP command reference](cli-reference/mcp.md) for client setup.

## Cloud sandboxes {#sandbox}

### Create a sandbox

Create a managed sandbox using your configured backend:

```sh
magic computer new
```

### Copy a file

Use the sandbox ID returned when you created it:

```sh
magic computer scp <sandbox-id> ./report.csv
```

The file is copied to the sandbox's working directory, `/computer` on current MagicMtn Canonical images. An existing file with the same name is replaced.

### Open a shell

List your sandboxes, then connect using a full ID or unique prefix:

```sh
magic computer list
magic computer ssh <id>
```

Press **Ctrl+]** to disconnect and leave the shell running. Run the same command to reconnect. Type `exit` to close the shell without stopping the sandbox.

### Terminate a sandbox

Terminate a sandbox and end its attached sessions:

```sh
magic computer terminate <id>
```

Enter `y` to confirm or press Enter to cancel. Save any files you need before terminating the sandbox.

See the [computer command reference](cli-reference/computer.md) for custom images, sandbox requirements, and scripting options.

## Completion

Install tab completion for your shell:

| Shell | Command                 |
| ----- | ----------------------- |
| Bash  | `magic completion bash` |
| Zsh   | `magic completion zsh`  |
| Fish  | `magic completion fish` |

The command asks before changing your shell configuration. Restart your shell when it finishes.

For Zsh, ensure `autoload -Uz compinit` followed by `compinit` runs after the `fpath` line added to `~/.zshrc`.

See the [completion reference](cli-reference/completion.md) for manual setup and PATH configuration.

## Providers

Providers connect your installation to local runtimes or services in your own infrastructure accounts. Follow [provider setup](providers.md) to prepare the provider you want to use.

List installed providers:

```sh
magic provider list
```

Install the Apple Container provider on macOS:

```sh
magic provider install apple-container
```

On Linux, install Firecracker:

```sh
magic provider install firecracker
```

To remove a provider, clean up its local sandboxes and stop its runner first:

```sh
magic provider uninstall apple-container
```

See the [provider command reference](cli-reference/provider.md) for package installation and other options.
