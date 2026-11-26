---
slug: /open-source
title: Open source MagicMtn
sidebar_label: Overview
---

Run MagicMtn on your computer or an internal host to manage sandboxes through your own providers. The open source server includes the CLI, Web UI, and SQLite storage for your installation.

## Start on your computer

[Install the CLI](../cli-basics.md#install), choose a sandbox provider, and start its runtime. For Apple Container on macOS, follow the [local server setup](../cli-reference/server.md#start). Start MagicMtn with `magic server start` and keep the server terminal open while you work.

In another terminal, prepare Canonical for your provider, create a sandbox, and run a command. The [computer reference](../cli-reference/computer.md) covers execution, shell access, inspection, and cleanup.

Open [http://localhost:1900](http://localhost:1900) and sign in with the dashboard credentials printed at startup. The [Web UI guide](../web-ui.md) explains Templates, Computers, and Images.

## Choose your providers

Providers connect your installation to compute and inference services. You can use a local runtime such as Apple Container or Firecracker, or a provider such as Modal with your own account.

[Install a provider](../providers.md), then follow its setup instructions in the [Registry](https://magicmtn.ai/registry). Use [provider configuration](../cli-reference/provider.md) to save credentials and prepare the connection for your installation.

## Run on an internal host

Install MagicMtn and your providers on the host that will operate the service. Start `magic server start` there. The server stores its database, configuration, and provider credentials on that host.

The server's API and Web UI listen on loopback. To open its Web UI from your workstation, forward port 1900 with SSH. Run `ssh -N -L 1900:127.0.0.1:1900 USER@HOST`, replacing `USER@HOST` with your SSH destination, then open [http://localhost:1900](http://localhost:1900) using that server's dashboard credentials.

Run CLI commands on the host through SSH. Keep the server running for sandbox inspection and cleanup, and preserve its [local storage](../cli-reference/server.md#local-storage) across restarts.

## Connect a coding agent

Use the [Codex](../coding-agents/codex.md) or [Claude Code](../coding-agents/claude-code.md) guide to work inside a local sandbox. Install the [Pi extension](../cli-reference/server.md#pi-on-your-computer) to let Pi use your installation's sandbox tools with your own inference credentials.
