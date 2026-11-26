---
title: CLI Reference
---

Use `magic` to work with MagicMtn Cloud or run a single-user sandbox server on your computer.

## Commands

- [login](cli-reference/login.md)
- [Pi](cli-reference/pi.md)
- [computer](cli-reference/computer.md)
- [server](cli-reference/server.md)
- [images](cli-reference/images.md)
- [provider](cli-reference/provider.md)
- [agents](cli-reference/agents.md)
- [mcp](cli-reference/mcp.md)
- [update](cli-reference/update.md)
- [whoami](cli-reference/whoami.md)
- [help](cli-reference/help.md)
- [completion](cli-reference/completion.md)
- [logout](cli-reference/logout.md)
- [uninstall](cli-reference/uninstall.md)

## Select a server

A successful `magic login` selects Cloud. Starting a [local server](cli-reference/server.md) selects local mode for the CLI and Pi extension, using its saved address and private credential automatically:

```sh
magic server start
# In another terminal:
magic computer list
```

| Option | Description |
| --- | --- |
| `--local` | Use the server started by `magic server start`. No Cloud login is required. Cannot be combined with `--server-url`. |
| `--server-url <URL>` | Select a server explicitly. Login credentials remain scoped to that URL. |

Stop the local server and run `magic login` to switch back to Cloud. Stopping the server alone keeps local mode selected.

An explicit `--server-url` selects a server for that CLI command. The selected consumer mode takes precedence over `MAGICMTN_SERVER_URL`; that environment variable supplies the default server before a mode is selected. `--local` explicitly selects the single-user server. Developer Cloud servers and their logins run independently of the selected consumer mode. The Pi extension follows the selected consumer mode.

The local server supports sandbox creation, execution, shell access, file transfer, inspection, and termination. In local mode, Pi runs on your computer with its own model credentials and MagicMtn sandbox tools. Use the local Web UI to browse the MCP Catalog and edit templates. Cloud provides managed agent workspaces, account inference, and gateway connections.
