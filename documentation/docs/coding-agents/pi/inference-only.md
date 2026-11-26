---
title: Inference only
---

Use MagicMtn inference with Pi running on your computer. Pi works with your local files and tools; no workspace sandbox is created.

## Set up Pi

Follow the [Pi setup guide](../pi.md#install-pi) to install Pi, the MagicMtn CLI, and the extension, then sign in. Open Pi in the directory where you want to work:

```sh
pi
```

## Select a model

In Pi, run:

```text
/magic inference
```

Choose a MagicMtn model for the current local Pi process. You can also open `/magic` and select **Inference only**.

Send your request in Pi as usual. Model requests use MagicMtn inference, while Pi's tools run on your computer.

## Save a default model

Run `/magic default` to save a default inference model on this computer for your account and organization.
