---
title: Claude Code
---

# Run Claude Code in a local sandbox

Use your Claude subscription to run Claude Code inside a MagicMtn sandbox on your Mac. Claude Code edits files and runs commands inside the sandbox. Sign in directly to Claude Code with your own account.

## Start MagicMtn

Install the [MagicMtn CLI](../cli-basics.md#install) and [Apple Container](https://github.com/apple/container), then start the local server with internet access for new sandboxes.

```sh
container system start
magic provider install apple-container
magic server start --network default
```

Keep this terminal open. If you already started the server without internet access, run `magic server stop`, then start it again with `--network default` before creating a sandbox.

## Create a sandbox and sign in

In another terminal, create a sandbox and open its shell.

```sh
magic --local computer new |
  xargs -o magic --local computer ssh
```

Inside the sandbox, start Claude Code.

```sh
claude
```

Choose to sign in with your Claude account and open the sign-in URL in your Mac’s browser. If the browser shows a login code, paste it into the Claude Code prompt in the sandbox. Clone or copy your project into the sandbox before working on it.

Exit Claude Code with `/exit`, then type `exit` to return to your Mac. Use `magic --local computer list` to find the sandbox ID and run `magic --local computer ssh <ID>` to reopen it. Run `claude --continue` from the same project directory to resume your most recent conversation there.

## Connect MCP tools

Run `claude mcp add` inside the sandbox to connect a tool server that Claude Code can reach from there. See [Claude Code’s MCP guide](https://code.claude.com/docs/en/mcp) for the command syntax and sign-in options.

If you run Claude Code on your Mac, connect it to the local MagicMtn server so it can create sandboxes, run commands, and read or write sandbox files through MCP.

```sh
claude mcp add --transport stdio \
  magicmtn -- magic --local mcp serve
claude mcp list
```

To connect to MagicMtn Cloud instead, sign in and register the Cloud server.

```sh
magic login
claude mcp add --transport stdio \
  magicmtn -- magic mcp serve
```

See the [MagicMtn MCP reference](../cli-reference/mcp.md) for details.

## Finish

Run `claude auth logout` inside the sandbox to sign out. Copy out any files you want to keep, then delete the sandbox from your Mac with `magic --local computer terminate <ID>`.

New sandboxes expire after 24 hours by default. See [local server settings](../cli-reference/server.md) for lifetime and cleanup options.
