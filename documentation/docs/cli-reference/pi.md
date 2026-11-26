---
title: Pi
---

Manage Pi workspaces and inference connections. Pi commands use full UUIDs for workspace and connection IDs. Commands other than `attach` print their results as JSON.

## list

List your Pi workspaces:

```sh
magic pi list
```

## create

Create a workspace using MagicMtn Canonical and a registered model:

```sh
magic pi create --model <MODEL_UUID>
```

To use a connected Pi inference profile, pass `--pi-connection` instead of `--model`.

| Flag | Required | Description |
| --- | --- | --- |
| `--model <MODEL>` | One of two | Registered model UUID. Cannot be combined with `--pi-connection`. |
| `--pi-connection <PI_CONNECTION>` | One of two | Connected Pi inference profile UUID. Cannot be combined with `--model`. |
| `--idempotency-key <UUID>` | No | Identifier for this creation request. Generated when omitted. |

If creation is interrupted, inspect `magic pi list` before creating another workspace. Reuse an idempotency key only for the same creation request.

## status

Show the durable agent-session status for a workspace:

```sh
magic pi status <ID>
```

## attach

Connect your terminal to Pi in a workspace:

```sh
magic pi attach <ID>
```

An interactive terminal is required. Press **Ctrl+]** to disconnect while leaving the workspace running.

## stop

Stop the agent without deleting its sandbox:

```sh
magic pi stop <ID>
```

## restart

Restart the agent using its saved conversation:

```sh
magic pi restart <ID>
```

## terminate

Terminate a workspace and delete its sandbox:

```sh
magic pi terminate <ID>
```

This command submits the termination request immediately, without a confirmation prompt.

## connections

List connected inference profiles without displaying credentials:

```sh
magic pi connections
```

## disconnect

Revoke access to a connected inference credential:

```sh
magic pi disconnect <ID>
```

Use the connection UUID from `magic pi connections`.
