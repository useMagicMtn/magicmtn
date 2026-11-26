---
title: Providers
---

Provider packages connect MagicMtn to the services and runtimes that run your workloads. Browse the [Registry](https://magicmtn.ai/registry) to find a provider and check its requirements and setup instructions.

## Install a provider

Install the [MagicMtn CLI](./cli-basics.md#install), then run the installation command shown on the provider's Registry page. For example, to install the Apple Container adapter on macOS:

```sh
magic provider install apple-container
```

Start the runtime listed in the provider’s requirements before installing its adapter. The command installs the adapter on your computer. Use `magic --local images prepare canonical --provider apple-container` to prepare Canonical for Apple Container after starting the local server. For Apple Container and Firecracker, see [Local sandbox setup](./coding-agents/sandbox-backends/local.md).

## List installed providers

```sh
magic provider list
```

## Remove a provider

Clean up the provider's local sandboxes and stop its runner before uninstalling it:

```sh
magic provider uninstall apple-container
```

See the [provider command reference](./cli-reference/provider.md) for package installation options and command details.
