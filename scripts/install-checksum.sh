#!/bin/sh
# Public installer: publish this file as install.sh for the current release.
set -eu

base_url='https://magicmtn-cli-993499103295.s3.us-east-2.amazonaws.com'
# Replaced with the current release identifier when publishing.
release='@RELEASE@'

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) platform=darwin-arm64 ;;
  Linux-x86_64) platform=linux-x86_64 ;;
  Linux-aarch64|Linux-arm64) platform=linux-arm64 ;;
  *) printf 'Error: supported platforms are Apple Silicon macOS and x86-64/ARM64 Linux.\n' >&2; exit 1 ;;
esac

modify_shell=ask
for argument in "$@"; do
  case "$argument" in
    --yes) modify_shell=yes ;;
    --no-modify-shell) modify_shell=no ;;
    *) printf 'Usage: install.sh [--yes | --no-modify-shell]\n' >&2; exit 1 ;;
  esac
done

install_dir=${MAGIC_INSTALL_DIR:-"$HOME/.local/bin"}
case "$install_dir" in
  /*) ;;
  *) printf 'Error: MAGIC_INSTALL_DIR must be an absolute path.\n' >&2; exit 1 ;;
esac
mkdir -p "$install_dir"
temporary=$(mktemp -d "$install_dir/.magic-install.XXXXXX")
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

printf 'Downloading MagicMtn CLI…\n'
curl -fsSL "$base_url/releases/$release/magic-$platform" -o "$temporary/magic"
curl -fsSL "$base_url/releases/$release/magic-$platform.sha256" -o "$temporary/magic.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$temporary" && sha256sum -c magic.sha256)
else
  (cd "$temporary" && shasum -a 256 -c magic.sha256)
fi
chmod 755 "$temporary/magic"
if ! "$temporary/magic" --version; then
  printf 'Error: the downloaded CLI could not run; the existing installation has not been changed.\n' >&2
  exit 1
fi
# Treat a directory at the destination as an error instead of moving into it.
if [ -d "$install_dir/magic" ]; then
  printf 'Error: %s/magic is a directory.\n' "$install_dir" >&2
  exit 1
fi
# Rename replaces an existing symlink without modifying its target.
mv -f "$temporary/magic" "$install_dir/magic"
printf '\nInstalled magic in %s\n' "$install_dir"
curl -q -fsS --max-time 2 -o /dev/null -X POST \
  -H 'X-MagicMtn-Installer: shell' https://magicmtn.ai/install-completed 2>/dev/null || :
if [ "$modify_shell" != no ]; then
  if [ "$modify_shell" = yes ]; then
    set -- --yes
  else
    set --
  fi
  if ! "$install_dir/magic" completion --path "$install_dir" "$@"; then
    printf 'Could not configure your shell; run magic completion --path "%s" to retry.\n' "$install_dir" >&2
  fi
fi
case ":$PATH:" in
  *":$install_dir:"*) printf 'Next: run magic login. For Pi, run magic agents install pi, then /reload in Pi.\n' ;;
  *) printf 'Next: run "%s/magic" login. For Pi, run "%s/magic" agents install pi, then /reload in Pi.\n' "$install_dir" "$install_dir" ;;
esac
