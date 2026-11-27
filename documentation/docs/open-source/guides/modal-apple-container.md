---
title: Modal inference + Apple Container VMs
slug: /open-source/guides/modal-apple-container
description: Run MagicMtn-managed Pi in a local Apple Container VM with inference in your own Modal account.
---

Use Modal for inference while Pi works inside a MagicMtn Canonical VM on your Mac. Canonical is the default machine image; MagicMtn manages Pi, the inference connection, and the sandbox lifecycle.

## Before you start

[Install the MagicMtn CLI](../../cli-basics.md#install) and [Apple Container](https://github.com/apple/container#get-started). Run the commands and Pi on your Mac. If you are signed in to MagicMtn Cloud, run `magic logout` to switch to your local installation.

## Install Modal and set up access

Install the Modal and Apple Container providers:

```sh
magic provider install modal
magic provider install apple-container
```

Create a [Modal workspace API token](https://modal.com/docs/guide/workspaces) with access to manage endpoints and Proxy Tokens. Save its values as `modal-credentials.json`:

```json
{
  "MODAL_TOKEN_ID": "YOUR_TOKEN_ID",
  "MODAL_TOKEN_SECRET": "YOUR_TOKEN_SECRET"
}
```

Start the local server in its own terminal and keep it open:

```sh
magic server start
```

In another terminal, configure Modal:

```sh
magic provider configure modal --secret-file ./modal-credentials.json
```

This establishes access to your Modal account. Model selection determines which inference resource MagicMtn provisions; the provider manages its endpoint and credentials.

## Prepare local isolation with Apple Container

Start the Apple Container runtime and make the Canonical image available to it:

```sh
container system start
magic images prepare canonical --provider apple-container
```

## Run MagicMtn-managed Pi

Install the Pi integration and open Pi:

```sh
magic agents install pi
pi
```

Run `/magic default` and select **Kimi K3** with **Modal** as the inference provider. Run `/magic new` and choose **Apple Container** as the sandbox backend.

MagicMtn starts Pi inside the local VM and routes its model requests to your Modal endpoint.

Ask Pi to work in its isolated workspace:

> Create a Python script in /computer that prints the first 20 Fibonacci numbers, run it, and show me the output.

## Disconnect and resume

Press **Ctrl+]** to disconnect while Pi keeps running. Use `/magic view` and choose **Resume** to reconnect, or **Terminate** to delete the workspace when finished.

## Try Modal sandboxes

Run the same Pi workflow inside a sandbox in your own Modal account. Reuse the configured Modal connection and selected model, and make the Canonical image available to Modal:

```sh
magic images prepare canonical --provider modal
pi
```

Keep the same model in `/magic default`. Run `/magic new` and choose **Modal** as the sandbox backend. MagicMtn runs Pi in the Modal sandbox and sends inference requests to the same model endpoint.

Use the same prompt, **Ctrl+]** to disconnect, and `/magic view` to resume or terminate the workspace.

## Clean up

Use `/magic view` and choose **Terminate** for each workspace you want to delete. After terminating your workspaces, stop the local server:

```sh
magic server stop
```

See the [Modal provider setup guide](https://github.com/useMagicMtn/provider-modal/blob/main/USAGE.md) for account and endpoint configuration.
