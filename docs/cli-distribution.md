# CLI distribution

MagicMtn releases authenticate their metadata with an ECDSA P-256 key pinned in `release/cli-trust.json`. The CLI embeds that key, and the independently deployed bootstrap installer pins the same key. The public installer is served independently of the download bucket at `https://magicmtn.ai/install.sh`.

The publisher signs the release's version, source commit, platform hashes, optional Apple identity, and validity period, then uploads immutable artifacts. It promotes the signed envelope to `latest.signed.json` only after the artifacts are available. The publisher also maintains `latest.json` in its original format so existing CLIs can upgrade to this verification path. Installation and `magic update` verify the signature, publisher, validity period, and executable hash before executing the download. When an Apple team ID is pinned, macOS also verifies the binary's Apple signing identity and the publisher requires notarization.

Run the release checks from the repository root:

```sh
python3 -m unittest discover -s scripts -p 'test_cli*.py'
cargo test --locked -p magicmtn-cli --bin magic release::tests
cargo test --locked -p magicmtn-cli --bin magic update::tests
```

## Configure the publisher

Generate a release key outside the checkout:

```sh
python3 scripts/cli_release.py keygen --private-key /secure/path/magicmtn-release.pem
python3 scripts/cli_release.py check-trust
```

Keep the private key in your signing system and commit only `release/cli-trust.json`. Source builds use that public trust configuration. Changes to the pinned key require a coordinated installer and CLI rollout.

For a local release, set `MAGICMTN_RELEASE_SIGNING_KEY_FILE` to the private-key path. Store a backup in your secret manager.

To additionally require Apple notarization, pin your ten-character Apple Developer team ID in `apple_team_id`. Set `MAGICMTN_APPLE_SIGNING_IDENTITY` to your Developer ID Application identity. Supply notarization access through `MAGICMTN_NOTARY_KEYCHAIN_PROFILE`, or through `MAGICMTN_NOTARY_KEY_FILE`, `MAGICMTN_NOTARY_KEY_ID`, and `MAGICMTN_NOTARY_ISSUER`.

The GitHub release workflow uses the `cli-release` environment. Configure `CLI_RELEASE_SIGNING_KEY_PEM` as an environment secret. A pinned Apple identity additionally requires `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_NOTARY_KEY_P8`, `APPLE_NOTARY_KEY_ID`, and `APPLE_NOTARY_ISSUER`. Its temporary signing keychain and credential files are deleted after the signing step.

## Publish and renew metadata

Build and publish from clean, committed source on Apple Silicon macOS:

```sh
python3 scripts/publish-cli.py --aws-profile magicmtn
```

For prebuilt artifacts, pass `--skip-build` to the publisher. When an Apple identity is pinned, first notarize `dist/bin/magic-darwin-arm64` with `scripts/cli_release.py notarize --binary dist/bin/magic-darwin-arm64 --receipt dist/bin/notarization.json`; the publisher independently checks Apple's acceptance before uploading. `--dry-run` prepares and authenticates the release without uploading or deleting objects.

Signed metadata is valid for fourteen days. The **Renew signed CLI metadata** workflow refreshes it weekly and supports manual dispatch. It authenticates the published source revision before checking it out and signing fresh validity dates.

To renew locally, use a clean checkout of the published source commit:

```sh
python3 scripts/publish-cli.py --refresh-metadata --aws-profile magicmtn
```

Renewal authenticates the existing manifest and checks its version and source commit before signing fresh validity dates. It updates only `latest.signed.json`; the version, binary hashes, and immutable release artifacts stay the same. Serialize renewal with release publication.

## Homebrew

The Homebrew synchronizer reads the publisher key from the independently served bootstrap, verifies the signed manifest, and generates the formula from its authenticated version and macOS checksum. The formula downloads the same immutable artifact as the installer and updater.

## GitHub releases

Write concise user-visible changes in `release/notes/VERSION.md` with each CLI version. The CLI publication workflow checks for notes before publishing and creates a GitHub release after successful publication. Its `vVERSION` tag points to the source commit authenticated by the signed manifest. Download links and checksums use the existing release bucket; its five-release retention policy also applies to these downloads.

To add the currently published version to GitHub Releases, dispatch **Publish GitHub CLI release** with that version, or run `python3 scripts/github-cli-release.py --version VERSION` with an authenticated `gh` CLI. Add `--dry-run` to verify public metadata and preview notes without creating a tag or release. Matching releases are left intact; conflicting tags or release notes cause the command to fail.
