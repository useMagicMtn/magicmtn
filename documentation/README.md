# Customer documentation

This Docusaurus site is served by the MagicMtn backend at `https://magicmtn.ai/docs/`. The proxy redirects `/docs` URLs on the control host to the same path on `magicmtn.ai`. Customer-facing Markdown lives in `documentation/docs/`. The existing repository `docs/` directory remains engineering documentation and is not published.

## Screen Recording

We use [asciinema](asciinema.org) to produce high-quality technical screen recording of the command line.

## Build and preview

Use Node.js 20 or later:

```sh
npm --prefix documentation ci
npm --prefix documentation run build
```

Run `npm --prefix documentation start` and open `http://localhost:3000/docs/` for a local preview. Rebuild after editing content when serving the generated `documentation/build` directory.

Set `DOCS_URL` at build time to change the canonical origin.

## Access and security

Anyone can open `/docs` and its nested guides without signing in. The backend serves the generated HTML, scripts, styles, search index, and attachments under `/docs/` with `Cache-Control: no-store`.

## Content policy

Write customer guidance as the intended product experience. Keep implementation status, roadmap caveats, internal architecture, and deployment secrets out of customer-facing pages. Track those details in engineering documentation. Update or remove affected guides in the same change that alters or removes a feature. Register published pages explicitly in `sidebars.js`; Docusaurus builds all files in its docs directory, so keep drafts and internal notes outside that directory. Broken page links fail the build.

All links leaving `/docs` must open in a new tab with `target="_blank"` and `rel="noopener noreferrer"`, including links to app routes on the same host. Use the `AppLink` component with root-relative paths in guides; it resolves app links against `https://control.magicmtn.ai`. Links within `/docs` stay in the current tab. The `plugins/link-policy.js` build hook checks every generated HTML page, including navigation, footer, Markdown, and custom HTML links, and fails the build for violations. Run `npm --prefix documentation test` for policy tests.

## Inspiring docs from other startups/projects

- https://docs.temporal.io/
- https://docs.litellm.ai/
