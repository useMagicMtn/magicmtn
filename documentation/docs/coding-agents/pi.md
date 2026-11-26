---
title: Pi
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import AsciinemaRecording from '@site/src/components/AsciinemaRecording';

The MagicMtn extension adds managed workspaces and inference to [Pi](https://pi.dev/). Install the latest Pi and the MagicMtn CLI.

## Install Pi

If you already have Pi installed, skip to [Install the MagicMtn CLI](#install-the-magicmtn-cli). Check your installed version with `pi --version`.

<Tabs defaultValue="macos">
<TabItem value="macos" label="macOS">

Open a terminal and run:

```sh
curl -fsSL https://pi.dev/install.sh | sh
```

</TabItem>
<TabItem value="nodejs" label="Node.js">

Install [Node.js](https://nodejs.org/en/download) 22.19 or newer, which includes npm. Then open a terminal and install the latest Pi:

```sh
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

</TabItem>
</Tabs>

Verify the installation:

```sh
pi --version
```

The command should print the installed version. See [Pi's official installation instructions](https://pi.dev/docs/latest/quickstart#1-install-pi) for additional installation methods and platform-specific setup.

## Install the MagicMtn CLI

Run:

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

The installer offers to configure your shell. Restart your terminal, then verify the installation:

```sh
magic --version
```

## Connect your account

Sign in and approve CLI access in your browser:

```sh
magic login
```

Install the extension and open Pi:

```sh
magic agents install pi
pi
```

If Pi is already running, restart it or use `/reload` to load the extension.

## Create a workspace

Run `/magic new` to create and enter a workspace with your default inference source and model. Use `/magic default` to change these defaults.

Pi runs inside the workspace sandbox. MagicMtn Canonical starts in `/computer` with its own home directory and Pi configuration. Clone your repositories and install any Pi extensions you need there.

Local files, credentials, conversations, skills, and extensions are not copied into the sandbox automatically. For GitHub repositories, see [GitHub](../integrations/github.md) to authorize your GitHub identity for agents in sandboxes you create.

### Watch: Set up Pi and start a background task

This recording installs the MagicMtn CLI, signs in, opens Pi, and creates a workspace. It then starts a task inside the workspace that continues after the local Pi connection closes.

<AsciinemaRecording
  src="/recordings/pi-setup-and-background-task.cast"
  label="Pi setup and background task terminal recording"
  poster="npt:2:45"
/>

## Disconnect and reconnect

Press **Ctrl+]** to return to local Pi. The remote Pi process keeps running while its relay grant remains valid.

Use `/magic view` to choose an existing workspace and reconnect. Reconnecting takes over the workspace terminal from an older connection.

### Watch: Verify the task from an SSH session

The follow-up recording connects to the same workspace over SSH and streams the file created by the Pi task. New lines keep appearing after the original Pi session has disconnected.

<AsciinemaRecording
  src="/recordings/pi-reconnect-and-stream.cast"
  label="SSH into the workspace and stream the task output terminal recording"
  poster="npt:0:50"
/>

## Use inference locally

To use MagicMtn models with your local files and tools, see [Inference only](./pi/inference-only.md).

## Delete a workspace

Use `/magic view`, choose the workspace, and select **Terminate**. Termination deletes the sandbox and its workspace files after confirmation. Save anything you need before confirming.
