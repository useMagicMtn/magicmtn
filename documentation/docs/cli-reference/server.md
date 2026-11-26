---
title: server
---

Run a single-user sandbox server on your computer with bundled SQLite. No MagicMtn account or separate database installation is required.

## start

Install a sandbox provider and prepare its host runtime before creating sandboxes. On macOS, install [Apple Container](https://github.com/apple/container), then run:

```sh
container system start
magic provider install apple-container
magic server start
```

Keep this terminal open. The server listens on `127.0.0.1:1924` and creates its database automatically. In another terminal, create a sandbox and run a command:

```sh
magic computer new |
  xargs -I {} magic computer exec {} -- /bin/sh -lc 'printf "hello from the sandbox\n"'
magic computer list
```

Canonical is included in the local image catalog. Prepare it with `magic --local images prepare canonical --provider apple-container`, then create a sandbox with `magic --local computer new`. Use [the image commands](images.md) to register custom images and prepare them for a provider.

A successful start selects local mode for the CLI and Pi extension. Commands use the server’s saved address and private credential automatically. No MagicMtn account, subscription, or login is required. See the [computer command reference](computer.md) for listing, execution, and termination options.

| Flag | Default | Description |
| --- | --- | --- |
| `--bind <ADDRESS>` | `127.0.0.1:1924` | Listener address and port. Must use a loopback IP address, such as `127.0.0.1:1925`. |
| `--network <NAME>` | Provider default | Override the Apple Container network for new sandboxes. Use `none` for isolation. |
| `--ttl-seconds <SECONDS>` | `86400` | Lifetime for new sandboxes, from 1 to 86400 seconds. |

Use an ID from `magic computer list` in `magic computer ssh <ID>` to open a shell, then type `exit` to return. Follow [Codex in a local sandbox](../coding-agents/codex.md) to use your ChatGPT subscription and MCP tools.

## Pi on your computer

Run `magic agents install pi`, then start Pi or run `/reload` in an existing Pi session. Pi keeps its own model and provider login. Use `/model` to select your model, `/magic new` to create a sandbox, and `/magic view` to manage sandboxes. MagicMtn’s sandbox tools operate through your local server; Pi’s built-in tools operate on your computer.

## Stop and restart

Press **Ctrl+C** in the server terminal to stop it. Running sandboxes remain available, and their records persist. Run `magic server start` again to resume inspection and cleanup. Expired sandboxes are cleaned up while the server is running; cleanup resumes after a restart.

To delete a sandbox and its files before stopping the server:

```sh
magic computer terminate <ID> --yes
```

Stopping the server keeps local mode selected. Run `magic server stop`, then `magic login` to switch the CLI and Pi extension to Cloud.

A developer’s Cloud server and its URL-scoped login run independently of this single-user server. Selecting a local sandbox provider in Cloud changes where workloads run and keeps the Cloud server selected.

Stop the server before installing or uninstalling providers. Keep provider versions used by active sandboxes until you terminate those sandboxes.

## Local storage

By default, the server stores `magicmtn.sqlite3` and its private `connection.json` credential in `~/.config/magicmtn/local`. Local CLI commands read that credential automatically. Keep the directory to retain your records and credential; to back it up, stop the server and copy the entire directory.

| Environment variable | Description |
| --- | --- |
| `MAGICMTN_CONFIG_DIRECTORY` | Change the configuration and installed-provider directory from `~/.config/magicmtn`. Local state defaults to its `local` subdirectory. |
| `MAGICMTN_LOCAL_DIRECTORY` | Override the database, credential, and lock directory. Set the same value for the server and local CLI commands. |

Each local directory supports one running server. This command uses bundled SQLite and has no PostgreSQL selection option.
