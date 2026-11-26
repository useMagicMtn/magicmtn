---
title: Slack
---

import AppLink from '@site/src/components/AppLink';

Connect Slack to request work from your General Agent by mentioning `@magic` in an allowed channel. Requests use your MagicMtn access; each member links their own Slack identity.

## Connect Slack

1. Open <AppLink href="/integrations">Integrations</AppLink> and select **Slack**.
2. Select **Install Slack app** and approve the installation in your Slack workspace. Skip this step if your workspace is already connected.
3. Return to the Slack integration page, select **Link my Slack identity**, and complete authorization.
4. Invite `@magic` to the Slack channel you want to use. Shared channels and Slack Connect are not supported.
5. Copy the channel ID from Slack's channel details. Enter it under **Allowed channels** on the integration page and select **Allow channel**.

## Request work

Mention `@magic` in an allowed channel with your request. For example:

```text
@magic Summarize the open issues in OWNER/REPOSITORY.
```

For repository work, first [authorize your GitHub account](./github.md#authorize-your-github-account).

Mention `@magic` again in the same thread to send follow-up work. Only the person who started the thread can steer or stop its agent. Other members can start their own threads with a new mention.

## Remove access

On the <AppLink href="/integrations/slack">Slack integration page</AppLink>, select **Unlink my identity** to remove your own access, or **Remove channel** to stop requests and agents in a channel. Select **Disconnect Slack and stop its agents** to disconnect the workspace.
