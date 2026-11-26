---
title: computer
---

Create, inspect, and connect to sandboxes. Sandbox IDs accept a full UUID or a unique prefix.

Commands use Cloud by default. For a [local server](server.md), add `--local`; local mode supports `new`, `exec`, `list`, `show`, and `terminate`.

## new

Create a managed sandbox using your configured backend:

```sh
magic computer new
```

The command prints the new sandbox ID to standard output. In Fish, use `read -x` to export it as `devbox`.

```fish
magic computer new | read -x devbox
```

To select an image explicitly, pass `--image`:

```sh
magic computer new --image <IMAGE> \
  --provider apple-container
```

| Flag | Required | Description |
| --- | --- | --- |
| `--image <IMAGE>` | No | Custom image available to the selected provider. Omit to use Canonical locally or managed creation in Cloud. |
| `--model <MODEL>` | No | Model UUID for managed creation. Defaults to your saved Pi preference. Cannot be combined with `--image` or `--pi-connection`. |
| `--pi-connection <PI_CONNECTION>` | No | Connected Pi inference profile UUID for managed creation. Cannot be combined with `--image` or `--model`. |
| `--provider <PROVIDER>` | No | Sandbox provider. Defaults to the host’s local provider in local mode. Cloud selections require `--image`. |
| `--connection <CONNECTION>` | No | Connection for an explicit image. Requires `--image`. |
| `--notes <NOTES>` | No | Notes to save with the sandbox. Cloud selections require `--image`. |

For managed creation, see [sandbox backend configuration](../coding-agents/config.md).

Local creation uses MagicMtn Canonical by default:

```sh
magic --local computer new
```

Canonical includes Python, Node.js, Pi, Codex, Git, Jujutsu, and GitHub CLI. Its exact digest is saved with the sandbox and retained across restarts. To use a custom image, add `--image alpine:3.24.1`.

## exec

Run a command in a sandbox and wait for its result:

```sh
magic computer exec <ID> --timeout-ms 10000 -- /bin/sh -lc 'printf "hello\n"'
```

For a local sandbox, use `magic --local computer exec` with the same arguments. The selected provider must support bounded execution. Use `--` before the command to separate its arguments from CLI options; invoke a shell explicitly when you need shell syntax such as pipes or redirection.

| Flag | Default | Description |
| --- | --- | --- |
| `--timeout-ms <TIMEOUT_MS>` | `30000` | Execution timeout, from 1 to 30000 milliseconds. |

The command prints JSON with `exit_code`, `stdout`, `stderr`, `elapsed_ms`, `timed_out`, and `truncated`. Output is limited to 16 KiB. The CLI exits unsuccessfully if the command fails or times out. Inspect the sandbox before retrying a command that changes state after a timeout or connection failure.

## list

List active sandboxes, including sandboxes created by agent sessions:

```sh
magic computer list
```

| Flag | Required | Description |
| --- | --- | --- |
| `--all` | No | Include sandboxes in every status. |

## show

Show details for a sandbox:

```sh
magic computer show <ID>
magic computer show <ID> --format json
```

| Flag | Required | Description |
| --- | --- | --- |
| `--format <FORMAT>` | No | Output format: `human` or `json`. Defaults to `human`. |

## ssh

Open an interactive shell in a live Cloud sandbox:

```sh
magic computer ssh <ID>
```

For a local server, use [`exec`](#exec) to run commands.

The connection uses MagicMtn's terminal relay. No SSH keys or inbound ports are required. The sandbox needs Python 3, a POSIX shell, and outbound access to MagicMtn.

Press **Ctrl+]** to disconnect and leave the shell running. Run the command again to reconnect. Type `exit` to close the shell without terminating the sandbox.

## scp

Copy a regular file from this computer into a Cloud sandbox:

```sh
magic computer scp <ID> ./report.csv
```

This command is unavailable with the local server.

The file keeps its name and permissions. The destination is the sandbox's working directory, `/computer` for current MagicMtn Canonical images. An existing file with the same name is replaced after size and checksum checks. The sandbox requires Python 3.

## terminate

Terminate a sandbox and end its attached sessions:

```sh
magic computer terminate <ID>
```

Enter `y` or `yes` to confirm. Press Enter to cancel. For noninteractive use, pass `--yes`.

| Flag | Required | Description |
| --- | --- | --- |
| `-y`, `--yes` | No | Skip the confirmation prompt. |
