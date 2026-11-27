#!/usr/bin/env python3
"""Shared, non-interactive CLI release signing and verification commands."""
import argparse
import cli_version
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import zipfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TRUST = ROOT / "release/cli-trust.json"
PLATFORMS = ("darwin-arm64", "linux-x86_64", "linux-arm64")
CHANNELS = ("alpha", "beta", "prerelease", "stable")
INSTALLER_URL = "https://control.magicmtn.ai/install.sh"
DOWNLOADS = "https://magicmtn-cli-993499103295.s3.us-east-2.amazonaws.com"
MAX_LIFETIME = 14 * 86400


def command(*args, **kwargs):
    kwargs.setdefault("stdin", subprocess.DEVNULL)
    return subprocess.run(list(map(str, args)), check=True, capture_output=True, **kwargs)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def trust(path=TRUST):
    value = json.loads(Path(path).read_text())
    if not isinstance(value, dict) or set(value) != {"public_key_pem", "apple_team_id"}:
        raise ValueError("Release trust must contain only the public key and Apple team ID")
    if not re.fullmatch(r"[A-Z0-9]{10}", value.get("apple_team_id", "")):
        raise ValueError("Configure apple_team_id in release/cli-trust.json before releasing")
    public = value.get("public_key_pem", "")
    with tempfile.TemporaryDirectory(prefix="magic-release-trust-") as directory:
        key = Path(directory) / "public.pem"
        key.write_text(public)
        der = command("openssl", "pkey", "-pubin", "-in", key, "-outform", "DER").stdout
        normalized = command("openssl", "pkey", "-pubin", "-in", key, "-pubout").stdout.decode()
        if public.replace("\r\n", "\n").strip() != normalized.strip():
            raise ValueError("The pinned public key must contain exactly one PEM public key")
    # SubjectPublicKeyInfo for id-ecPublicKey / prime256v1, followed by a SEC1 point.
    prefix = bytes.fromhex("3059301306072a8648ce3d020106082a8648ce3d030107034200")
    if len(der) != 91 or not der.startswith(prefix + b"\x04"):
        raise ValueError("The pinned release public key must be an ECDSA P-256 key")
    return value


def validate_payload(payload, now=None, allow_expired=False):
    now = int(time.time()) if now is None else now
    fields = {"schema", "release", "channel", "source_commit", "issued_at", "expires_at", "platforms", "apple_team_id"}
    if not isinstance(payload, dict) or set(payload) != fields:
        raise ValueError("Invalid signed release metadata fields")
    if type(payload.get("schema")) is not int or payload.get("schema") != 1 or payload.get("channel") not in CHANNELS:
        raise ValueError("Unsupported signed release schema or channel")
    if not isinstance(payload.get("release"), str) or not re.fullmatch(r"[A-Za-z0-9.-]{1,100}", payload["release"]):
        raise ValueError("Invalid signed release identifier")
    if payload["channel"] == "stable":
        cli_version.parse(payload["release"])
    if not isinstance(payload.get("source_commit"), str) or not re.fullmatch(r"[0-9a-f]{40}", payload["source_commit"]):
        raise ValueError("A signed release must identify its source commit")
    issued, expires = payload.get("issued_at"), payload.get("expires_at")
    if (type(issued) is not int or type(expires) is not int
            or issued > now + 300 or (expires <= now and not allow_expired)
            or not 0 < expires - issued <= MAX_LIFETIME):
        raise ValueError("Signed release metadata is expired or has invalid dates")
    hashes = payload.get("platforms", {})
    if not isinstance(hashes, dict) or set(hashes) != set(PLATFORMS) or any(
            not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value)
            for value in hashes.values()):
        raise ValueError("Signed releases require hashes for all supported platforms")
    return payload


def verify_envelope(data, configuration, now=None, allow_expired=False):
    envelope = json.loads(data)
    if not isinstance(envelope, dict) or set(envelope) != {"payload", "signature"}:
        raise ValueError("A signed release envelope is required; unsigned releases are rejected")
    payload = base64.b64decode(envelope["payload"], validate=True)
    signature = base64.b64decode(envelope["signature"], validate=True)
    with tempfile.TemporaryDirectory(prefix="magic-release-verify-") as directory:
        directory = Path(directory)
        (directory / "public.pem").write_text(configuration["public_key_pem"])
        (directory / "payload").write_bytes(payload)
        (directory / "signature").write_bytes(signature)
        command("openssl", "dgst", "-sha256", "-verify", directory / "public.pem",
                "-signature", directory / "signature", directory / "payload")
    # Only publisher renewal permits expired, authenticated metadata.
    parsed = validate_payload(json.loads(payload), now, allow_expired)
    if parsed.get("apple_team_id") != configuration["apple_team_id"]:
        raise ValueError("The signed release uses an unexpected Apple publisher")
    return parsed


def verify_artifacts(payload, directory):
    for platform in PLATFORMS:
        if digest(Path(directory) / ("magic-" + platform)) != payload["platforms"][platform]:
            raise ValueError("Release artifact does not match its signed hash: " + platform)


def sign_payload(payload, private_key, configuration):
    validate_payload(payload)
    if Path(private_key).expanduser().resolve().is_relative_to(ROOT):
        raise ValueError("Store the private signing key outside the repository")
    with tempfile.TemporaryDirectory(prefix="magic-release-sign-") as directory:
        directory = Path(directory)
        data = canonical(payload)
        (directory / "payload").write_bytes(data)
        signature = command("openssl", "dgst", "-sha256", "-sign", private_key,
                            directory / "payload").stdout
    envelope = json.dumps({"payload": base64.b64encode(data).decode(),
                           "signature": base64.b64encode(signature).decode()}, separators=(",", ":")) + "\n"
    verify_envelope(envelope, configuration)
    return envelope


def render_installer(configuration):
    source = (ROOT / "scripts/install-public.sh").read_text()
    return source.replace("@RELEASE_PUBLIC_KEY@", configuration["public_key_pem"].strip()).replace("@APPLE_TEAM_ID@", configuration["apple_team_id"])


def verify_bootstrap(configuration):
    with urllib.request.urlopen(INSTALLER_URL, timeout=30) as response:
        if response.geturl() != INSTALLER_URL or response.read() != render_installer(configuration).encode():
            raise ValueError("Deploy the matching authenticated bootstrap installer before publishing")


def notary_credentials():
    profile = os.environ.get("MAGICMTN_NOTARY_KEYCHAIN_PROFILE")
    if profile:
        return ["--keychain-profile", profile]
    names = ("MAGICMTN_NOTARY_KEY_FILE", "MAGICMTN_NOTARY_KEY_ID", "MAGICMTN_NOTARY_ISSUER")
    values = [os.environ.get(name) for name in names]
    if not all(values):
        raise ValueError("Set a notary keychain profile or all MAGICMTN_NOTARY_KEY_* / ISSUER settings")
    return ["--key", values[0], "--key-id", values[1], "--issuer", values[2]]


def verify_macos_signature(binary, configuration):
    requirement = 'anchor apple generic and certificate leaf[subject.OU] = "' + configuration["apple_team_id"] + '"'
    command("codesign", "--verify", "--strict", "-R", requirement, binary)
    details = command("codesign", "--display", "--verbose=4", binary).stderr.decode()
    if ("Authority=Developer ID Application:" not in details or "(runtime)" not in details
            or "Timestamp=" not in details):
        raise ValueError("macOS releases require Developer ID, hardened runtime, and a secure timestamp")


def verify_notarization(binary, receipt_path, configuration):
    verify_macos_signature(binary, configuration)
    receipt = json.loads(Path(receipt_path).read_text())
    archive = Path(receipt_path).parent / "magic-darwin-arm64.zip"
    if receipt.get("binary_sha256") != digest(binary):
        raise ValueError("The notarization receipt belongs to another binary")
    if receipt.get("archive_sha256") != digest(archive):
        raise ValueError("The notarization archive changed")
    with zipfile.ZipFile(archive) as zipped:
        if zipped.namelist() != ["magic"] or zipped.read("magic") != Path(binary).read_bytes():
            raise ValueError("The notarization archive must contain exactly the release binary")
    credentials = notary_credentials()
    # Re-read Apple's result; a local receipt is not proof of notarization.
    info = json.loads(command("xcrun", "notarytool", "info", receipt["id"],
                              *credentials, "--output-format", "json").stdout)
    if info.get("status") != "Accepted":
        raise ValueError("Apple has not accepted this notarization submission")
    log = json.loads(command("xcrun", "notarytool", "log", receipt["id"], *credentials).stdout)
    if log.get("status") != "Accepted" or log.get("sha256") != digest(archive):
        raise ValueError("Apple's notarization result does not match the release archive")
    return receipt


def notarize(binary, receipt_path, identity, configuration):
    binary, receipt_path = Path(binary), Path(receipt_path)
    credentials = notary_credentials()
    try:
        verify_macos_signature(binary, configuration)
    except (subprocess.CalledProcessError, ValueError):
        if not identity:
            raise ValueError("Set MAGICMTN_APPLE_SIGNING_IDENTITY to a Developer ID identity")
        command("codesign", "--force", "--options", "runtime", "--timestamp", "--sign", identity, binary)
        verify_macos_signature(binary, configuration)
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    archive = receipt_path.parent / "magic-darwin-arm64.zip"
    receipt = json.loads(receipt_path.read_text()) if receipt_path.exists() else {}
    if receipt.get("binary_sha256") != digest(binary) or not archive.exists() or receipt.get("archive_sha256") != digest(archive):
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
            zipped.write(binary, "magic")
        submitted = json.loads(command("xcrun", "notarytool", "submit", archive,
                                       *credentials, "--output-format", "json").stdout)
        receipt = {"id": submitted["id"], "binary_sha256": digest(binary), "archive_sha256": digest(archive)}
        # Keep the submission ID before waiting, so retries reuse this submission.
        receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    command("xcrun", "notarytool", "wait", receipt["id"], *credentials, "--timeout", "20m")
    verify_notarization(binary, receipt_path, configuration)
    return receipt


def source_commit():
    if command("jj", "diff", "--summary", "-r", "@", cwd=ROOT).stdout.strip():
        raise ValueError("Release source must be committed and clean")
    return command("jj", "log", "-r", "@-", "--no-graph", "-T", "commit_id", cwd=ROOT).stdout.decode().strip()


def prepare(directory, output, channel, configuration):
    directory, output = Path(directory), Path(output)
    if channel != "stable":
        raise ValueError("CLI publication uses the stable channel")
    verify_notarization(directory / "magic-darwin-arm64", directory / "notarization.json", configuration)
    hashes = {name: digest(directory / ("magic-" + name)) for name in PLATFORMS}
    source = source_commit()
    now = int(time.time())
    payload = {"schema": 1, "release": cli_version.current(ROOT), "channel": channel, "source_commit": source,
               "issued_at": now, "expires_at": now + MAX_LIFETIME, "platforms": hashes,
               "apple_team_id": configuration["apple_team_id"]}
    validate_payload(payload)
    output.mkdir(parents=True, exist_ok=True)
    (output / "payload.json").write_bytes(canonical(payload))
    return payload


def keygen(private_key, team):
    private_key = Path(private_key).expanduser().resolve()
    if json.loads(TRUST.read_text()).get("public_key_pem"):
        raise ValueError("A release key is already pinned; key rotation requires a separate rollout")
    if private_key.is_relative_to(ROOT):
        raise ValueError("Store the private signing key outside the repository")
    if not re.fullmatch(r"[A-Z0-9]{10}", team):
        raise ValueError("Supply the ten-character Apple team ID")
    descriptor = os.open(private_key, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as key:
            key.write(command("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256").stdout)
        public = command("openssl", "pkey", "-in", private_key, "-pubout").stdout.decode()
        TRUST.write_text(json.dumps({"public_key_pem": public, "apple_team_id": team}, indent=2) + "\n")
    except BaseException:
        private_key.unlink(missing_ok=True)
        raise
    print("Created the private key outside the repository; commit release/cli-trust.json before building")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    sub.add_parser("check-trust")
    sub.add_parser("check-bootstrap")
    key = sub.add_parser("keygen")
    key.add_argument("--private-key", type=Path, required=True)
    key.add_argument("--apple-team-id", required=True)
    mac = sub.add_parser("notarize")
    mac.add_argument("--binary", type=Path, required=True)
    mac.add_argument("--receipt", type=Path, required=True)
    mac.add_argument("--identity", default=os.environ.get("MAGICMTN_APPLE_SIGNING_IDENTITY"))
    create = sub.add_parser("prepare")
    create.add_argument("--artifacts", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    create.add_argument("--channel", choices=CHANNELS, required=True)
    sign = sub.add_parser("sign")
    sign.add_argument("--payload", type=Path, required=True)
    sign.add_argument("--private-key", type=Path, required=True)
    sign.add_argument("--output", type=Path, required=True)
    verify = sub.add_parser("verify")
    verify.add_argument("--manifest", type=Path, required=True)
    verify.add_argument("--artifacts", type=Path)
    render = sub.add_parser("render-installer")
    render.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.operation == "keygen":
        keygen(args.private_key, args.apple_team_id)
        return
    configuration = trust()
    if args.operation == "check-bootstrap":
        verify_bootstrap(configuration)
    elif args.operation == "notarize":
        notarize(args.binary, args.receipt, args.identity, configuration)
    elif args.operation == "prepare":
        prepare(args.artifacts, args.output, args.channel, configuration)
    elif args.operation == "sign":
        args.output.write_text(sign_payload(json.loads(args.payload.read_bytes()), args.private_key, configuration))
    elif args.operation == "verify":
        payload = verify_envelope(args.manifest.read_bytes(), configuration)
        if args.artifacts:
            verify_artifacts(payload, args.artifacts)
    elif args.operation == "render-installer":
        args.output.write_text(render_installer(configuration))
    print("CLI release " + args.operation + " passed")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        # Do not print subprocess output or commands containing signing credentials.
        raise SystemExit("CLI release failed: " + (str(error) if not isinstance(error, subprocess.CalledProcessError) else "a required verification/signing command failed")) from None
