---
slug: /faq
title: FAQ
---

import AppLink from '@site/src/components/AppLink';

## What is MagicMtn?

MagicMtn provides inference, routing, and sandboxed compute for agents. [MagicMtn Cloud](./cloud/index.md) manages the service and resources for you. [Open source MagicMtn](./open-source/index.md) runs on your own infrastructure with providers you install.

## How do I run MagicMtn myself?

[Install the CLI](./cli-basics.md#install), install a provider, and start the server with `magic server start`. Use the Web UI at [http://localhost:1900](http://localhost:1900) to inspect your installation. Follow the [open source guide](./open-source/index.md) for developer machines and internal hosts.

## MagicMtn Cloud

### What can I do on the platform today?

Start long-running agent sessions from a coding harness such as [Pi](./coding-agents/pi.md) or [OpenCode](./coding-agents/opencode.md), or from [Slack](./integrations/slack.md). You can also [create standalone sandboxes](./cli-reference/computer.md), [define custom machine templates](./templates.md), and [connect MCP tools](./mcp.md) for your agents.

### Is MagicMtn for me or my team?

Both. You can start on your own or create an organization and invite teammates from <AppLink href="/members">Members</AppLink>. Your team uses a shared credit balance with no per-seat charges.

### Do I need accounts with infrastructure providers?

No. MagicMtn provisions managed compute and gives you access to cloud sandboxes, VMs, and models through its providers.

### Can I use my own inference provider or model subscription?

Yes. You can run agents on MagicMtn-managed compute and configure them to use your existing inference provider or model subscription. MagicMtn's managed inference is optional.

### Do agents receive my tool credentials?

For tools connected through MagicMtn's managed gateway, the underlying credentials stay outside the sandbox. The gateway authenticates requests to connected services. You choose which repositories and services your agent can access. See [Security](./security.md) for details.

### Will my agent keep working when I disconnect?

Yes. Sessions started in managed cloud workspaces from Pi, OpenCode, or the MagicMtn app can keep running when you disconnect or shut down your computer.

### What happens to an idle machine?

MagicMtn pauses idle machines and expires them after prolonged inactivity or when they reach their configured lifetime.

### What happens to my data?

MagicMtn retains session content for the duration of a task and does not use your data for training. MagicMtn may temporarily keep snapshots to help agents complete tasks and deletes those snapshots later.

### Why the emphasis on the Pi harness?

Pi was the first coding harness we integrated. Its extension system lets us add managed workspaces and model selection directly to the terminal workflow. You can also use MagicMtn with [OpenCode](./coding-agents/opencode.md).

### Why would I use Pi+MagicMtn instead of just Pi?

Pi sessions run in managed cloud sandboxes, so agents can keep working when you disconnect or shut down your computer.

You choose which repositories and services the sandbox can access. Grant only the access your agent needs.

### How do I get started with Pi?

Follow the [Pi setup guide](./coding-agents/pi.md) to install the CLI and start your first managed workspace.
