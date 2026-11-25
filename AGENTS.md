# Agent instructions

## Documentation and copywriting

Do not add caveats to copy anywhere. Things such as "not enabled yet", "not connected yet", et cetera. Copy should always assume that a feature is active.

## Coding style

Use [Google Rust Style Guide] for Rust code.

## Markdown

Keep each Markdown prose paragraph or list item on one source line and let the editor soft-wrap it.

## CLI releases

Follow [the CLI release guide](../cloud/docs/cli-releases.md) when producing a new CLI version or changing its release tooling.

- Use stable Semantic Versioning starting at `1.0.0`: patch for compatible fixes, minor for compatible features, and major for breaking changes to public commands, flags, configuration, or machine-readable output.
- Every published release needs a new version. Group development changes into releases; do not bump the version for every edit or reuse a published version, even after its artifacts have been pruned.
- Treat `cli/Cargo.toml` as the version source of truth. Run `python3 scripts/cli_version.py patch`, `minor`, or `major` to update it and the CLI entry in `Cargo.lock` together. Record the version with the source changes it describes.
- Verify `magic --version` matches the intended release version on every supported platform. Run the release tests and compatibility checks required by the release guide before publishing.
- Publish from clean, committed source with `scripts/publish-cli.py` or the CLI release workflow. Serialize publication, preserve the version and source commit in release metadata, and never overwrite published artifacts.
- Use `magicmtn-cli-993499103295` for the installer, release artifacts, and new CLI updates. Keep only the latest five successful releases, deleting older artifacts after successful promotion through the publisher's retention logic.
- Keep the legacy bucket only as an update bridge for existing CLIs. Refresh its copy of the current release and remove its previous artifacts; it must not accumulate a separate release history.

[Google Rust Guide]: https://google.github.io/styleguide/rustguide.html
