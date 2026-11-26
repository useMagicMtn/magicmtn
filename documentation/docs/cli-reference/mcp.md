---
title: mcp
---

Expose sandbox tools to compatible MCP clients.

## serve

Start the MCP server over standard input and output:

```sh
magic mcp serve
```

Run `magic login` first. Configure your MCP client to launch `magic` with the arguments `mcp` and `serve`. Standard output is reserved for MCP messages.
