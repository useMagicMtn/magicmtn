---
title: Templates
---

import AppLink from '@site/src/components/AppLink';

Use the `default` template to clone the GitHub repositories you use regularly into new MagicMtn cloud sandboxes. Each account or organization workspace has its own template.

## Add repositories to your template

1. [Connect and authorize GitHub](./integrations/github.md#authorize-your-github-account) in the workspace where you'll create sandboxes. Include the repositories you want the template to clone.
2. Open <AppLink href="/templates">Templates</AppLink> and add your repositories under `[repos]` in **Template TOML**. Replace `acme/api` and `acme/shared` with your repositories.

   ```toml
   name = "default"
   description = "Backend"

   [repos]
   github = [
     "acme/api",
     "acme/shared",
   ]
   ```

3. Select **Save template**, then [create a sandbox](./cli-reference/computer.md#new).

After your sandbox starts, MagicMtn clones each accessible repository into the sandbox's working directory, under `/computer/<repo>` on current MagicMtn Canonical images in the background. If a repository is missing, check that your GitHub authorization is active and that both your account and the GitHub App have access to it.

Use `owner/repo` for up to 10 repositories, each with a distinct repository name so they have separate directories. The description is optional and can be up to 160 characters.
