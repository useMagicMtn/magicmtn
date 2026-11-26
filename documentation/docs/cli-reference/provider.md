---
title: provider
---

Manage provider packages installed on this computer. See [Local sandbox setup](../coding-agents/sandbox-backends/local.md) for host runtime and runner setup.

For standalone use without a Cloud account, see the [local server reference](server.md). Stop the local server before installing or uninstalling providers.

## list

List locally installed providers:

```sh
magic provider list
magic provider list --format json
```

| Flag                | Required | Description                                            |
| ------------------- | -------- | ------------------------------------------------------ |
| `--format <FORMAT>` | No       | Output format: `human` or `json`. Defaults to `human`. |

## install

Install the latest provider package for your operating system and architecture from the MagicMtn Registry:

```sh
magic provider install apple-container
```

To install a package from disk, use `--file` instead of a provider name:

```sh
magic provider install --file ./provider.tar.gz
```

| Flag               | Required | Description                                                        |
| ------------------ | -------- | ------------------------------------------------------------------ |
| `--file <PACKAGE>` | No       | Local package to install. Cannot be combined with a provider name. |

Installation verifies and stores the package locally, including any bundled private runtime and dependencies. Runtime checks must pass before the release becomes active. With the local server running, use `magic images prepare canonical --provider apple-container` to prepare Canonical for Apple Container. Install the host virtualization runtime separately. Listing and installing packages do not require sign-in.

## configure

With the local server running, configure the installed provider using its credential JSON:

```sh
magic provider configure modal --secret-file ./modal-credentials.json
magic provider configure modal --secret-file ./modal-credentials.json --option app=my-workspaces
```

The provider validates the credentials and creates or selects the account resources it needs. Modal defaults to an app named `magicmtn` and saves its immutable app ID. Repeat the command to rotate credentials for the same account. Use `--option KEY=VALUE` for provider-specific setup options; repeat the flag for multiple options. `--secret-string JSON` is an alternative to `--secret-file PATH`.

## verify

Check the saved connection without creating resources:

```sh
magic provider verify modal
```

## uninstall

Remove all locally installed versions of a provider:

```sh
magic provider uninstall apple-container
```

Clean up the provider's local sandboxes and stop its runner first. If this computer has registered local connections, the command checks for remaining sandbox references before removal.
