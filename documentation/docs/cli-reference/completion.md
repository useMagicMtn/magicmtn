---
title: completion
---

Install tab completion for Bash, Zsh, or Fish:

```sh
magic completion [bash|zsh|fish]
```

When the shell is omitted, the CLI detects it. Installation asks before changing shell configuration. Restart your shell after installing completions.

| Flag | Required | Description |
| --- | --- | --- |
| `--print` | No | Print the completion script without changing shell configuration. |
| `--path <PATH>` | No | Also add an absolute directory to your shell's PATH. Cannot be combined with `--print`. |
| `--yes` | No | Allow shell configuration changes without prompting. Cannot be combined with `--print`. |

To print a script for manual setup:

```sh
magic completion zsh --print
```
