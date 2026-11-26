---
title: MCP
---

import AppLink from '@site/src/components/AppLink';

MCP lets agents in MagicMtn sandboxes use tools from services you authorize. Open the <AppLink href="/mcp">MCP Catalog</AppLink> to see which services you can connect.

## Connect a service

1. In the MCP Catalog, choose a service and select **Connect**.
2. Follow its authorization flow and choose the accounts and permissions you want to grant. Some services require additional setup shown on their catalog cards.
3. Return to the catalog and confirm that the service appears under **Connected MCPs**.

Connections are shared with your MagicMtn workspace, but each member must authorize their own access. Your authorization applies to sandboxes you create; it does not give other members access through your account.

## Use MCP tools in a sandbox

In a sandbox with [Pi](./coding-agents/pi.md), ask the agent to find and use a connected service's tools. Pi uses `mcp_catalog` to discover available tools and `mcp_call` to run one. A connection added while a sandbox is running becomes available on its next catalog lookup.

The sandbox does not receive your service's OAuth token or API key. MagicMtn checks your authorization when it forwards each tool call. The service's own account permissions still determine what the tool can do.

## Remove access

In the <AppLink href="/mcp">MCP Catalog</AppLink>, remove your access to a connected service to stop your sandboxes from using it. Other members who authorized the same connection keep their own access.
