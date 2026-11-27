#!/usr/bin/env python3
"""Check public CLI installation and an older updater in disposable directories."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import urllib.request

import cli_version

DOWNLOADS = "https://magicmtn-cli-993499103295.s3.us-east-2.amazonaws.com"


def download(url):
    with urllib.request.urlopen(url, timeout=120) as response:
        return response.read()


def check_hash(binary, expected):
    if hashlib.sha256(binary).hexdigest() != expected:
        raise ValueError("The public binary does not match its release checksum")


def run(executable, *arguments, environment):
    result = subprocess.run([str(executable), *arguments], env=environment, check=True,
                            capture_output=True, text=True, timeout=180)
    print(result.stdout.strip(), flush=True)
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upgrade-from", required=True, help="An existing retained version to upgrade")
    args = parser.parse_args()
    cli_version.parse(args.upgrade_from)
    targets = {("Darwin", "arm64"): "darwin-arm64", ("Linux", "aarch64"): "linux-arm64",
               ("Linux", "x86_64"): "linux-x86_64"}
    target = targets.get((platform.system(), platform.machine()))
    if not target:
        raise ValueError("Run on a supported native CLI platform")
    latest = json.loads(download(DOWNLOADS + "/latest.json"))
    version = latest["release"]
    cli_version.parse(version)
    if cli_version.parse(args.upgrade_from) >= cli_version.parse(version):
        raise ValueError("Choose an upgrade source older than the public release")
    expected = latest["platforms"][target]
    with tempfile.TemporaryDirectory(prefix="magic-cli-public-smoke-") as temporary:
        root = Path(temporary)
        home, install = root / "home", root / "bin"
        home.mkdir()
        environment = {key: value for key, value in os.environ.items()
                       if key in ("PATH", "TMPDIR", "LANG", "SSL_CERT_FILE", "SSL_CERT_DIR")}
        environment.update(HOME=str(home), MAGIC_INSTALL_DIR=str(install),
                           MAGICMTN_CONFIG_DIRECTORY=str(root / "configuration"))
        installer = root / "install.sh"
        installer.write_bytes(download("https://magicmtn.ai/install.sh"))
        run("sh", str(installer), "--no-modify-shell", environment=environment)
        executable = install / "magic"
        check_hash(executable.read_bytes(), expected)
        if run(executable, "--version", environment=environment) != "magic " + version:
            raise ValueError("The installer selected a different release")
        run(executable, "update", environment=environment)
        check_hash(executable.read_bytes(), expected)
        old_prefix = DOWNLOADS + "/releases/" + args.upgrade_from
        old = json.loads(download(old_prefix + "/release.json"))
        binary = download(old_prefix + "/magic-" + target)
        check_hash(binary, old["platforms"][target])
        executable.write_bytes(binary)
        executable.chmod(0o755)
        if run(executable, "--version", environment=environment) != "magic " + args.upgrade_from:
            raise ValueError("The upgrade source has the wrong version")
        run(executable, "update", environment=environment)
        check_hash(executable.read_bytes(), expected)
        if run(executable, "--version", environment=environment) != "magic " + version:
            raise ValueError("The older updater did not install the current release")
        run(executable, "--help", environment=environment)
        print(f"Passed {target}: fresh install, current update, and {args.upgrade_from} -> {version}", flush=True)


if __name__ == "__main__":
    main()
