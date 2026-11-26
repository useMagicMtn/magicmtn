# Documentation writing instructions

Write for a person trying to get something done. Respect their attention: give them the next useful action and enough context to understand it.

These rules apply to technical documentation across this repo. Customer pages live in `documentation/docs/`; internal engineering docs live in `docs/`. Follow [README.md](README.md) for publishing, links, and build requirements.

## Choose what belongs

- Give each page a clear purpose. Open with one or two sentences explaining what the reader can do or learn. Skip introductions that merely announce the page.
- In getting-started guides, show the recommended path first. Include prerequisites the reader must act on, then get to the first useful command.
- Assume macOS readers use Apple Silicon. Do not add architecture explanations or compatibility caveats to ordinary setup instructions. Use platform tabs when the actual steps differ.
- Put exhaustive flags, alternative configurations, and uncommon behaviors in reference or troubleshooting pages. Link to the relevant section where useful.
- Include a caveat beside a step only when it changes what the reader should do or prevents a likely failure, data loss, or security problem. State the consequence and remedy directly. Avoid inventories of hypothetical exceptions.
- Keep implementation mechanics, internal access checks, release history, and roadmap commentary out of customer guides unless they affect the user's task. Verify behavior before describing it; brevity must not create false promises.
- Keep the MCP guide provider-neutral. Describe the shared workflow for connecting, using, and removing services. Adding an MCP server does not require a Docs update. Put the list of available servers and service-specific setup instructions in the MCP Catalog in the app. Add service-specific Docs only when explicitly requested.

## Paragraphs and line breaks

- Give each paragraph one idea, usually in one to three sentences. Aim for about 20–60 words; review paragraphs over 80 words for cuts or a natural split. These are editing heuristics, not quotas or measured Temporal standards.
- Start a new paragraph when moving from an action to its result, an alternative, or a different concept. Keep sentences that explain the same idea together.
- Put a blank line between paragraphs and around headings, lists, and code blocks. A single newline in Markdown source does not create a rendered paragraph break.
- Keep each prose paragraph or list item on one source line and let your editor soft-wrap it. Avoid `<br>`, trailing spaces, and repeated blank lines for visual spacing.
- Cut excess detail before splitting a long paragraph. A wall of short paragraphs can still be too much text.

## Make pages easy to scan

- Use short, descriptive headings in sentence case. Prefer task headings such as “Create a sandbox.” Use `##` for sections and `###` for necessary subsections.
- Keep the action, example, and result together: a brief instruction, a code block, then a short explanation of what happens or how to check success when useful.
- Use numbered lists for ordered steps, bullets for parallel items, and tables for comparable fields or options. Use prose for explanations. Avoid deeply nested lists.
- Use fenced code blocks with language labels and copyable commands. Explain placeholders nearby. Separate alternatives so readers do not run all of them.
- Use inline code for commands, flags, paths, and identifiers. Reserve bold for selective emphasis or UI labels. Use callouts sparingly for consequential advice.
- Rely on the site's typography and spacing. Avoid custom HTML or page-specific styling for ordinary prose. Link meaningful phrases instead of “click here.”

## Sound like a helpful colleague

Use “you,” active voice, familiar words, and direct verbs: “Run,” “Open,” “Create.” Explain unfamiliar product concepts when they first matter. Assume technical competence without assuming knowledge of MagicMtn internals.

Remove filler such as “It is important to note,” “simply,” “please be aware,” and “the following command can be used to.” Avoid repeating the heading, narrating every flag, or restating the same behavior before and after an example.

For example, replace:

> The login command initiates a browser-based authentication flow. Once the browser has opened, you will need to authenticate and approve the CLI access request before returning to the terminal to continue using the CLI.

With:

Sign in:

```sh
magic login
```

Approve CLI access in your browser, then return to your terminal.

## Before finishing

Read the page once as a first-time user. Can you find the next action by scanning the headings and examples? Remove sentences that do not help you act, understand the result, or make a necessary decision. Move optional detail to its proper home.

Check commands and links. For published page changes, run the documentation build and inspect the rendered page at desktop and narrow widths when preview is available. Look for dense paragraphs, detached explanations, and overflowing code or tables. Report unavailable checks honestly.

## Reference examples

Temporal's [CLI basics](https://docs.temporal.io/cli/common-operations) demonstrates short task sections and focused commands. Its [CLI setup](https://docs.temporal.io/cli/setup-cli) uses platform tabs and separates installation from configuration. The [Workflow overview](https://docs.temporal.io/workflows) and [TypeScript timeouts guide](https://docs.temporal.io/develop/typescript/workflows/timeouts) show how conceptual explanations and code examples can be grouped under headings. Use these structural patterns with judgment; some sections are denser than our preferred style. The rules above are this repository's standard.
