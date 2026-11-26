---
title: OpenCode
---

Use OpenCode with MagicMtn to run coding sessions in managed sandboxes or use MagicMtn models with the files and tools on your computer.

## Install the integration

Install [OpenCode](https://opencode.ai/docs/#cli) 2.3.2 or later and the [MagicMtn CLI](../cli-basics.md#install). Sign in, approve CLI access in your browser, then install the integration and start OpenCode.

```sh
magic login
magic agents install opencode
opencode
```

If OpenCode is already running, restart it to load the integration. Type `/magic` to open the workspace menu.

## Create a workspace

Run `/magic new` to create and enter a managed workspace. OpenCode starts in `/computer` on MagicMtn Canonical. Use `/magic default` to choose the model for new workspaces. For a connected API-key provider, use `/magic connected` to choose its model for a new workspace.

Clone your project inside the sandbox. Use the [GitHub integration](../integrations/github.md) to access your repositories, and ask OpenCode to discover tools from your [connected MCP services](../mcp.md).

## Disconnect and reconnect

Press **Ctrl+]** to return to your local OpenCode session while work continues in the sandbox. Run `/magic view` and choose a workspace to reconnect to its conversation.

Use the same menu to stop or restart the agent. To delete the sandbox and its files, choose **Terminate sandbox** and confirm. Save anything you need before terminating.

## Use MagicMtn inference with local files

Run `/magic inference` and select a MagicMtn model in OpenCode's model picker. Your OpenCode session continues to use the files and tools on your computer.

## Use local sandbox tools

If you run the [local MagicMtn server](../cli-reference/server.md), start that server to select local mode, then install the integration.

```sh
magic agents install opencode
opencode
```

OpenCode uses your selected model. Use `/magic new` to create a local sandbox or `/magic view` to open a sandbox shell or terminate a sandbox. You can also ask OpenCode to list, create, inspect, or terminate sandboxes, run commands, and read or write files inside them.
