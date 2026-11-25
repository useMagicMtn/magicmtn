#!/usr/bin/env python3
"""Read or bump the stable CLI version in Cargo's package and lock files."""
import argparse
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def parse(version):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("CLI releases require a stable MAJOR.MINOR.PATCH version")
    return tuple(map(int, version.split(".")))


def current(root=ROOT):
    version = tomllib.loads((root / "cli/Cargo.toml").read_text())["package"]["version"]
    parse(version)
    return version


def bump(part, root=ROOT):
    old = current(root)
    major, minor, patch = parse(old)
    new = {"major": f"{major + 1}.0.0", "minor": f"{major}.{minor + 1}.0", "patch": f"{major}.{minor}.{patch + 1}"}[part]
    package = root / "cli/Cargo.toml"
    lock = root / "Cargo.lock"
    package_text, count = re.subn(r'(?m)^version = "' + re.escape(old) + r'"$', f'version = "{new}"', package.read_text(), count=1)
    lock_text, lock_count = re.subn(r'(name = "magicmtn-cli"\nversion = ")' + re.escape(old) + '"', lambda match: match[1] + new + '"', lock.read_text())
    if count != 1 or lock_count != 1:
        raise ValueError("CLI package and Cargo.lock versions must agree before bumping")
    package.write_text(package_text)
    lock.write_text(lock_text)
    return new


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bump", nargs="?", choices=("major", "minor", "patch"))
    args = parser.parse_args()
    print(bump(args.bump) if args.bump else current())


if __name__ == "__main__":
    main()
