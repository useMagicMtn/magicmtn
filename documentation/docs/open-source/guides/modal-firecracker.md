---
title: Modal inference with Firecracker isolation
slug: /open-source/guides/modal-firecracker
description: Run MagicMtn-managed Pi in a local Firecracker VM with inference in your own Modal account.
---

Run MagicMtn-managed Pi inside a local Firecracker microVM and send its inference requests to Modal. MagicMtn Canonical is the default machine image; MagicMtn manages Pi, the model endpoint, and the sandbox lifecycle.

## Before you start

[Install the MagicMtn CLI](../../cli-basics.md#install) on your Linux host. Set up Firecracker, access to `/dev/kvm`, Python, and OpenSSH using the [Firecracker provider's host and guest configuration guide](https://github.com/useMagicMtn/provider-firecracker/blob/main/USAGE.md#host-and-guest-configuration). Prepare a Canonical kernel and ext4 root filesystem, SSH access, and a TAP network slot using that guide. Save the provider configuration as `$HOME/.config/magicmtn/firecracker.json`.

Run the commands and Pi on that Linux host. Have your Modal workspace token ready. If you are signed in to MagicMtn Cloud, run `magic logout` to switch to your local installation.

## Install Modal and set up access

Install the Modal and Firecracker providers:

```sh
magic provider install modal
magic provider install firecracker
```

Create a [Modal workspace API token](https://modal.com/docs/guide/workspaces) with access to manage endpoints and Proxy Tokens. Save its values as `modal-credentials.json`:

```json
{
  "MODAL_TOKEN_ID": "YOUR_TOKEN_ID",
  "MODAL_TOKEN_SECRET": "YOUR_TOKEN_SECRET"
}
```

In a terminal you will keep open, select the Firecracker configuration and start the local server:

```sh
export MAGICMTN_FIRECRACKER_CONFIG="$HOME/.config/magicmtn/firecracker.json"
magic server start
```

In another terminal on the same host, configure Modal:

```sh
magic provider configure modal --secret-file ./modal-credentials.json
```

This establishes access to your Modal account. Selecting a model provisions its inference endpoint and credentials.

## Prepare local isolation with Firecracker

Make the prepared Canonical VM image available to MagicMtn:

```sh
magic images prepare canonical --provider firecracker
```

MagicMtn validates the configured Canonical kernel and root filesystem and saves the image selection for the Firecracker backend. Each new computer gets its own writable copy of the root filesystem and a prepared network slot.

## Run MagicMtn-managed Pi

Install the Pi integration and open Pi:

```sh
magic agents install pi
pi
```

Run `/magic default` and select **Kimi K3** with **Modal** as the inference provider. Run `/magic new` and choose **Firecracker** as the sandbox backend.

MagicMtn starts Pi inside the local Firecracker VM and routes model requests to your Modal endpoint. Pi's commands and file edits run inside the VM, with `/computer` as the working directory.

Ask Pi to work in its isolated workspace:

> Create a Python script in /computer that prints the first 20 Fibonacci numbers, run it, and show me the output.

## Disconnect and resume

Press **Ctrl+]** to disconnect while Pi keeps running. Open Pi again, run `/magic view`, select the computer, and choose **Resume** to return to the same workspace and conversation.

## Clean up

Use `/magic view` and choose **Terminate** to delete the VM and its workspace files and release its network slot. After terminating your computers, stop the local server:

```sh
magic server stop
```

The selected Modal inference endpoint is reused across computers. Terminating a computer keeps the inference connection available for your next session.
