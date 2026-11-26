---
title: Sandbox backends
---

A sandbox backend provides the isolated machine where your coding agent runs commands and stores project files. MagicMtn keeps track of its lifecycle and routes commands to the same sandbox throughout the session.

By default, new coding sandboxes use automatic cloud placement. Enable [Local](local.md) to run sandboxes on your own computer using Apple Container on macOS or Firecracker on Linux. The [Config](../config.md) page describes the switch.

Local placement changes where code executes. It does not change the coding agent's inference provider or automatically copy your host repositories and credentials into the sandbox. Keep the local runner running for command execution and cleanup; shutting down your computer makes its sandboxes unavailable.
