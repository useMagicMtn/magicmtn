---
title: MagicMtn Web UI
---

The MagicMtn Web UI lets you inspect computers and images and configure the repositories your agents use in new sandboxes. It is included with the MagicMtn CLI.

Start your [local server](./cli-reference/server.md#start) with `magic server start`, then open [http://localhost:1900](http://localhost:1900). Sign in as `magic` using the dashboard password printed in the terminal. When you redirect server output, the startup message gives the path to a private file containing the password.

Keep the server running while you use the Web UI. The dashboard displays the computers, images, and settings stored by that MagicMtn installation.

## Templates

Open **Templates** to edit the `default` template. Add public GitHub repositories as `owner/repo` in the TOML editor and select **Save template**. MagicMtn clones those repositories into `/workspace` inside new computers.

The template supports up to ten repositories and an optional description of up to 160 characters. Use repository names with distinct directory names. Changes apply to computers you create after saving.

## Computers

Open **Computers** to see your sandboxes, their status, creation time, image, and notes. Use this view to check which computers belong to your installation.

Create computers, run commands, open an SSH session, and terminate computers through the [computer commands](./cli-reference/computer.md). Refresh the page to see their latest state.

## Images

Open **Images** to browse the container images registered in your installation. Each image shows its source and preparation status for your providers.

MagicMtn includes Canonical in the image catalog. Use the [image commands](./cli-reference/images.md) to prepare an image for a provider or add your own container image, then create a computer from it.
