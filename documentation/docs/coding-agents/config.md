---
title: Config
---

MagicMtn reads preferences from `~/.config/magicmtn/config`, a TOML file on macOS and Linux.

Create the directory:

```sh
mkdir -p ~/.config/magicmtn
```

To place Cloud-managed sandboxes on your computer, [set up a local runner](sandbox-backends/local.md), then create `~/.config/magicmtn/config` with:

```toml
local-sandbox = true
```

| Option | Type | Default | Effect |
| --- | --- | --- | --- |
| `local-sandbox` | Boolean | `false` | Create coding sandboxes on this machine: Apple Container on macOS, Firecracker on Linux. |
| `error-reporting` | Boolean | `true` | Send Pi session errors to Sentry in the background. |

If the file or option is missing, MagicMtn uses cloud placement.

The setting applies to `magic computer new` and Pi's `/magic new`. Existing sandboxes keep their backend. Selecting an image and provider explicitly overrides this preference.

Set `local-sandbox = false` to use automatic cloud placement for new coding sandboxes. The local runner preserves the selected placement. Your inference settings are independent of sandbox placement.

Server selection is independent of this preference. `magic login` selects the Cloud server, including when `local-sandbox = true`. `magic server start` selects the single-user local server for the CLI and Pi extension. Stop that server and run `magic login` to switch back.

## Pi error reporting

Pi session errors are reported in the background. To turn reporting off, add this setting to `~/.config/magicmtn/config` on the computer you use to launch Pi:

```toml
error-reporting = false
```

The preference applies when you create or reconnect to a Pi workspace. It suppresses Pi model, tool, compaction, startup, and unexpected exit reports. Other applications inside the sandbox keep their own error reporting settings.

Reports contain failure categories and session identifiers. They exclude prompts, transcripts, tool arguments and output, credentials, and raw error messages.
