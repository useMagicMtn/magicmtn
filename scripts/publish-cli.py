#!/usr/bin/env python3
"""Build and publish authenticated standalone CLI releases."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import cli_version
import cli_release as release

ROOT = Path(__file__).resolve().parents[1]
BUCKET = "magicmtn-cli-993499103295"
PLATFORMS = ("darwin-arm64", "linux-x86_64", "linux-arm64")


def prepare(artifacts, output, channel, signing_key=None):
    if channel != "stable":
        raise ValueError("CLI publication uses the stable channel")
    version = cli_version.current(ROOT)
    hashes = {}
    for platform in PLATFORMS:
        binary = artifacts / ("magic-" + platform)
        if not binary.is_file() or not binary.stat().st_size:
            raise ValueError("Missing or empty artifact: " + str(binary))
        hashes[platform] = hashlib.sha256(binary.read_bytes()).hexdigest()
    identifier = version
    source = subprocess.check_output(["jj", "log", "-r", "@-", "--no-graph", "-T", "commit_id"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["jj", "diff", "--summary", "-r", "@"], cwd=ROOT))
    manifest = {"release": identifier, "version": version, "channel": channel, "published_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "platforms": hashes, "source_commit": source, "source_dirty": dirty}
    if dirty:
        raise ValueError("Commit CLI source changes before publishing")
    actual = subprocess.check_output([str((artifacts / "magic-darwin-arm64").resolve()), "--version"], text=True).strip()
    if actual != "magic " + version:
        raise ValueError(f"Built CLI reports {actual!r}; expected magic {version}")
    configuration = release.trust()
    signing_key = signing_key or os.environ.get("MAGICMTN_RELEASE_SIGNING_KEY_FILE")
    if not signing_key:
        raise ValueError("Supply --signing-key or MAGICMTN_RELEASE_SIGNING_KEY_FILE")
    release.verify_notarization(artifacts / "magic-darwin-arm64", artifacts / "notarization.json", configuration)
    now = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
    payload = {"schema": 1, "release": identifier, "channel": channel, "source_commit": source,
               "issued_at": now, "expires_at": now + release.MAX_LIFETIME,
               "platforms": hashes, "apple_team_id": configuration["apple_team_id"]}
    signed = release.sign_payload(payload, signing_key, configuration)
    output.mkdir(parents=True, exist_ok=True)
    (output / "manifest.json").write_text(signed)
    (output / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
    shutil.copyfile(ROOT / "LICENSE", output / "LICENSE")
    for platform, digest in hashes.items():
        (output / ("magic-" + platform + ".sha256")).write_text(digest + "  magic\n")
    (output / "SHA256SUMS").write_text("".join(digest + "  magic-" + platform + "\n" for platform, digest in hashes.items()))
    (output / "install.sh").write_text(release.render_installer(configuration))
    return manifest


def release_objects(aws):
    # AWS CLI automatically paginates; do not limit this to the first 1,000 objects.
    data = subprocess.check_output([*aws, "s3api", "list-objects-v2", "--bucket", BUCKET,
                                    "--prefix", "releases/", "--region", "us-east-2", "--output", "json"], text=True)
    return json.loads(data).get("Contents", [])


def validate_publication(version, objects):
    target = cli_version.parse(version)
    for entry in objects:
        parts = entry["Key"].split("/")
        if len(parts) < 3:
            continue
        if parts[1] == version:
            raise ValueError(f"Version {version} already exists; bump the CLI version before publishing")
        if parts[2] == "release.json":
            try:
                previous = cli_version.parse(parts[1])
            except ValueError:
                continue  # Timestamp releases predate semantic versioning.
            if previous >= target:
                raise ValueError(f"Version {version} must be newer than published version {parts[1]}")


def prune_releases(aws, current):
    latest = release.verify_envelope(subprocess.check_output([*aws, "s3", "cp", f"s3://{BUCKET}/latest.signed.json", "-", "--region", "us-east-2"], text=True), release.trust())
    if latest["release"] != current:
        raise ValueError("Current release is not the live latest release; refusing retention cleanup")
    objects = release_objects(aws)
    completed = {}
    prefixes = set()
    for entry in objects:
        parts = entry["Key"].split("/")
        if len(parts) < 3 or not parts[1]:
            continue
        prefixes.add(parts[1])
        if parts[2] == "release.json":
            completed[parts[1]] = entry["LastModified"]
    if current not in completed:
        raise ValueError("Current release metadata is missing; refusing retention cleanup")
    # The promoted release always counts toward the five, even if clocks differ.
    keep = {current, *sorted((name for name in completed if name != current),
                            key=lambda name: (completed[name], name), reverse=True)[:4]}
    for name in sorted(prefixes - keep):
        subprocess.run([*aws, "s3", "rm", f"s3://{BUCKET}/releases/{name}/", "--recursive",
                        "--region", "us-east-2", "--only-show-errors"], check=True)


def refresh_metadata(aws, output, signing_key, dry_run=False):
    configuration = release.trust()
    if not signing_key:
        raise ValueError("Supply --signing-key or MAGICMTN_RELEASE_SIGNING_KEY_FILE")
    data = subprocess.check_output([*aws, "s3", "cp", f"s3://{BUCKET}/latest.signed.json", "-", "--region", "us-east-2"], text=True)
    payload = release.verify_envelope(data, configuration, allow_expired=True)
    if payload["channel"] != "stable" or payload["release"] != cli_version.current(ROOT) or payload["source_commit"] != release.source_commit():
        raise ValueError("Check out the published source commit before renewing release metadata")
    now = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
    payload.update(issued_at=now, expires_at=now + release.MAX_LIFETIME)
    signed = release.sign_payload(payload, signing_key, configuration)
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / "manifest.json"
    manifest.write_text(signed)
    if not dry_run:
        release.verify_bootstrap(configuration)
        subprocess.run([*aws, "s3", "cp", str(manifest), f"s3://{BUCKET}/latest.signed.json", "--region", "us-east-2",
                        "--content-type", "application/json", "--cache-control", "no-cache", "--only-show-errors"], check=True)
    print("Renewed signed metadata for release " + payload["release"] + (" (dry run)" if dry_run else ""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refresh-metadata", action="store_true", help="Renew the current signed manifest from its clean source checkout without rebuilding or replacing release artifacts")
    parser.add_argument("--skip-build", action="store_true", help="Publish previously built and checked artifacts")
    parser.add_argument("--artifacts", type=Path, default=ROOT / "dist/bin")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/public-release")
    parser.add_argument("--channel", choices=("stable",), default="stable")
    parser.add_argument("--aws-profile")
    parser.add_argument("--signing-key", type=Path, default=os.environ.get("MAGICMTN_RELEASE_SIGNING_KEY_FILE"))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    os.environ["AWS_CLI_AUTO_PROMPT"] = "off"
    os.environ["AWS_PAGER"] = ""
    aws = ["aws"] + (["--profile", args.aws_profile] if args.aws_profile else [])
    if args.refresh_metadata:
        refresh_metadata(aws, args.output, args.signing_key, args.dry_run)
        return
    if not args.skip_build:
        for script in ("package-cli.sh", "package-cli-linux.sh", "check-cli-linux.sh"):
            subprocess.run(["sh", "scripts/" + script], cwd=ROOT, check=True)
        shutil.copy2(args.artifacts / "magic", args.artifacts / "magic-darwin-arm64")
        configuration = release.trust()
        release.notarize(args.artifacts / "magic-darwin-arm64", args.artifacts / "notarization.json",
                         os.environ.get("MAGICMTN_APPLE_SIGNING_IDENTITY"), configuration)
    payload = prepare(args.artifacts, args.output, args.channel, args.signing_key)
    if args.dry_run:
        print("Release prepared and checksums generated; no objects uploaded")
        return
    def upload(path, key, content_type, cache):
        if key.startswith("releases/"):
            subprocess.run([*aws, "s3api", "put-object", "--bucket", BUCKET, "--key", key,
                            "--body", str(path), "--if-none-match", "*", "--region", "us-east-2",
                            "--content-type", content_type, "--cache-control", cache], check=True,
                           stdout=subprocess.DEVNULL)
        else:
            subprocess.run([*aws, "s3", "cp", str(path), f"s3://{BUCKET}/{key}",
                            "--region", "us-east-2", "--content-type", content_type,
                            "--cache-control", cache, "--only-show-errors"], check=True)

    release.verify_bootstrap(release.trust())
    objects = release_objects(aws)
    validate_publication(payload["version"], objects)
    prefix = "releases/" + payload["release"] + "/"
    immutable = "public,max-age=31536000,immutable"
    for platform in PLATFORMS:
        name = "magic-" + platform
        upload(args.artifacts / name, prefix + name, "application/octet-stream", immutable)
        upload(args.output / (name + ".sha256"), prefix + name + ".sha256", "text/plain", immutable)
    upload(args.output / "SHA256SUMS", prefix + "SHA256SUMS", "text/plain", immutable)
    upload(args.output / "LICENSE", prefix + "LICENSE", "text/plain; charset=utf-8", immutable)
    upload(args.output / "manifest.json", prefix + "manifest.json", "application/json", immutable)
    upload(args.output / "release.json", prefix + "release.json", "application/json", immutable)
    # Promote only after every immutable artifact has uploaded successfully.
    upload(args.output / "manifest.json", "latest.signed.json", "application/json", "no-cache")
    # Older CLIs read the original manifest shape to upgrade to signature enforcement.
    upload(args.output / "release.json", "latest.json", "application/json", "no-cache")
    upload(args.output / "install.sh", "install.sh", "text/plain; charset=utf-8", "no-cache")
    prune_releases(aws, payload["release"])
    print(f"Published {args.channel} release {payload['release']}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit("CLI publication failed: " + str(error)) from None
