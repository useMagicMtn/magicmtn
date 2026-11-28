#!/usr/bin/env python3
"""Create a GitHub release for the authenticated, published CLI."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import urllib.request

import cli_release as release
import cli_version

REPOSITORY = "useMagicMtn/magicmtn"
ROOT = Path(__file__).resolve().parents[1]


def notes_for(version):
    cli_version.parse(version)
    notes = (ROOT / "release/notes" / (version + ".md")).read_text().strip()
    if not notes:
        raise ValueError("Write user-visible release notes before publishing")
    return notes


def download(path):
    with urllib.request.urlopen(release.DOWNLOADS + "/" + path, timeout=60) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("Release metadata exceeds the size limit")
    return data


def prepare(version, notes):
    cli_version.parse(version)
    payload = release.verify_envelope(download("latest.signed.json"), release.trust())
    if payload["release"] != version or payload["channel"] != "stable":
        raise ValueError("Select the currently published stable CLI version")
    prefix = "releases/" + version + "/"
    published = json.loads(download(prefix + "release.json"))
    if any(published.get(key) != payload[key] for key in
           ("release", "channel", "source_commit", "platforms")) or published.get("version") != version or published.get("source_dirty") is not False:
        raise ValueError("Published release metadata differs from the signed manifest")
    checksums = download(prefix + "SHA256SUMS").decode()
    expected = {"magic-" + platform: digest for platform, digest in payload["platforms"].items()}
    lines = checksums.splitlines()
    if len(lines) != len(expected) or dict(line.split("  ")[::-1] for line in lines) != expected:
        raise ValueError("Published checksums differ from the signed manifest")
    base = release.DOWNLOADS + "/" + prefix
    downloads = [(f"[{platform}]({base}magic-{platform})", f"`{payload['platforms'][platform]}`")
                 for platform in release.PLATFORMS]
    width = max(len(link) for link, _ in downloads)
    table = "\n".join([f"| {'Platform':<{width}} | {'SHA-256':<66} |",
                       f"| {'-' * width} | {'-' * 66} |",
                       *(f"| {link:<{width}} | {digest} |" for link, digest in downloads)])
    body = f"""{notes}

## Install or update

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh
```

For an existing installation, run `magic update`.

## Downloads

{table}

[Checksums]({base}SHA256SUMS) · [Signed manifest]({base}manifest.json) · [Release metadata]({base}release.json)

Source: [{payload['source_commit']}](https://github.com/{REPOSITORY}/commit/{payload['source_commit']})
"""
    return payload, body


def api(path):
    result = subprocess.run(["gh", "api", "repos/" + REPOSITORY + "/" + path],
                            capture_output=True, text=True)
    if result.returncode:
        if "(HTTP 404)" in result.stderr:
            return None
        raise subprocess.CalledProcessError(result.returncode, result.args, result.stdout, result.stderr)
    return json.loads(result.stdout)


def publish(payload, body):
    tag = "v" + payload["release"]
    reference = api("git/ref/tags/" + tag)
    if reference is not None:
        target = reference["object"]
        for _ in range(5):
            if target["type"] != "tag":
                break
            target = api("git/tags/" + target["sha"])["object"]
        if target["type"] != "commit" or target["sha"] != payload["source_commit"]:
            raise ValueError("Existing release tag differs from the authenticated source commit")
    existing = api("releases/tags/" + tag)
    if existing is not None:
        if reference is None or existing["draft"] or existing["prerelease"] or (existing.get("body") or "").strip() != body.strip():
            raise ValueError("Existing GitHub release differs; refusing to overwrite it")
        print(existing["html_url"])
        return
    with tempfile.TemporaryDirectory(prefix="magic-github-release-") as temporary:
        path = Path(temporary) / "notes.md"
        path.write_text(body)
        subprocess.run(["gh", "release", "create", tag, "--repo", REPOSITORY,
                        "--target", payload["source_commit"], "--title", "MagicMtn CLI " + payload["release"],
                        "--notes-file", str(path), "--latest"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", default=cli_version.current(ROOT))
    parser.add_argument("--check-notes", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    notes = notes_for(args.version)
    if args.check_notes:
        return
    payload, body = prepare(args.version, notes)
    if args.dry_run:
        print(body)
    else:
        publish(payload, body)


if __name__ == "__main__":
    main()
