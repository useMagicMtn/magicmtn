---
slug: /cloud
title: MagicMtn Cloud
sidebar_label: Overview
---

import AppLink from '@site/src/components/AppLink';

MagicMtn Cloud provisions and automatically scales cloud compute for your agents. Use machine templates to set up new sandboxes and managed MCP to connect agents to the tools they need.

## Launch a cloud sandbox

[Create a standalone sandbox](../cli-reference/computer.md#new) from the CLI or start one from your coding harness. For inference, choose a model from the <AppLink href="/models">managed model catalog</AppLink> or [use your own provider or model subscription](../faq.md#can-i-use-my-own-inference-provider-or-model-subscription).

## Connect tools with managed MCP

[Connect services such as Linear, Sentry, or Notion](../mcp.md) from the MCP Catalog. The [MagicMtn gateway](../security.md#credentials-and-the-gateway) checks your authorization when it forwards tool calls and keeps service credentials outside the sandbox.

## Set up machines with templates

Use a [machine template](../templates.md) to specify which GitHub repositories MagicMtn clones into new cloud sandboxes. [Authorize the repositories](../integrations/github.md#authorize-your-github-account) once so new machines can start with the code your agents need.

## Start a cloud coding session

Set up [Pi](../coding-agents/pi.md) or [OpenCode](../coding-agents/opencode.md), then run `/magic new` to open a cloud workspace. Ask your agent to run a test suite, disconnect with **Ctrl+]**, and return later with `/magic view`. The agent keeps working in the sandbox while you're away.

[Watch a Pi session keep working after disconnect](../coding-agents/pi.md#watch-set-up-pi-and-start-a-background-task).

[Connect Slack](../integrations/slack.md) to start a task with an `@magic` mention and steer it from the same thread.
