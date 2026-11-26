---
title: GitHub
---

import AppLink from '@site/src/components/AppLink';

Connect GitHub so agents in your sandboxes can clone repositories, push changes, and work with issues and pull requests.

Install the GitHub App on your personal or organization's GitHub account and select the repositories to share. Each MagicMtn member then authorizes their own GitHub account. Personal installations are visible only to their owner in MagicMtn.

## Authorize your GitHub account

1. Open <AppLink href="/integrations">Integrations</AppLink> and select **GitHub**.
2. Find the connected GitHub account or organization and select **Authorize my GitHub account**.
3. Approve the request on GitHub, then return to MagicMtn. The connection should say **Authorized as** your GitHub username.

Agents can access only the selected repositories that your GitHub account and the App's permissions allow. If your organization restricts App authorization, a GitHub administrator may need to approve it.

Signing in to MagicMtn with GitHub does not authorize repository access. Complete the steps above; no GitHub token needs to be pasted into the sandbox.

## Use GitHub from a sandbox

Use Git or the installed GitHub CLI, `gh`, from a sandbox you created. For example, clone a repository and list its issues. Replace `OWNER/REPOSITORY` with your repository:

```sh
gh repo clone OWNER/REPOSITORY
gh issue list --repo OWNER/REPOSITORY
```

GitHub HTTPS and SSH style remotes are routed to the gateway for cloning, fetching, and pushing. Set Git `user.name` and `user.email` in the repository if you want commits to show your chosen author identity. GitHub authenticates pushes as the GitHub account you authorized.

Git and repository operations use MagicMtn's gateway; your GitHub token stays outside the sandbox. Some other `gh` commands use unsupported endpoints and may fail.

If you need to select a specific App installation, set `MAGICMTN_GITHUB_CONNECTION_ID`.

## Remove access

On the GitHub connection page, select **Remove my GitHub authorization from this workspace** to stop gateway access for sandboxes you created. You can also revoke the App's authorization in GitHub. If your authorization expires or is revoked, return to the connection page to authorize your account again.
