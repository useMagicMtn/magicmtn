#!/bin/sh
# Exercise both libc families before publishing, including Alpine sandboxes.
set -eu
cd "$(dirname "$0")/.."
repository=$(pwd -P)
architectures='amd64 arm64'
version=$(python3 scripts/cli_version.py)
case "${1:-}" in
  --native)
    case "$(uname -m)" in x86_64) architectures=amd64 ;; aarch64|arm64) architectures=arm64 ;; *) exit 1 ;; esac
    ;;
  '') ;;
  *) printf 'Usage: check-cli-linux.sh [--native]\n' >&2; exit 1 ;;
esac
for arch in $architectures; do
  case "$arch" in
    amd64) binary=magic-linux-x86_64 ;;
    arm64) binary=magic-linux-arm64 ;;
  esac
  for distribution in alpine:3.24 debian:bookworm-slim; do
    printf 'Checking %s on %s (%s)\n' "$binary" "$distribution" "$arch"
    podman run --rm --platform "linux/$arch" \
      -v "$repository/dist/bin:/cli:ro" "docker.io/library/$distribution" \
      sh -ec '
        test "$("$1" --version)" = "magic $2"
        "$1" --help >/dev/null
        "$1" mcp serve --help >/dev/null
        "$1" completion bash --print >/tmp/completion
        test -s /tmp/completion
        MAGICMTN_CONFIG_DIRECTORY=/tmp/magicmtn-check "$1" server start --bind 127.0.0.1:0 >/tmp/local-server.log 2>&1 &
        server_pid=$!
        trap "kill -TERM $server_pid 2>/dev/null || true" EXIT
        for attempt in $(seq 1 100); do
          test ! -s /tmp/magicmtn-check/local/connection.json || break
          kill -0 "$server_pid" 2>/dev/null || { cat /tmp/local-server.log; exit 1; }
          sleep 0.1
        done
        test -s /tmp/magicmtn-check/local/connection.json || { cat /tmp/local-server.log; exit 1; }
        test -s /tmp/magicmtn-check/local/magicmtn.sqlite3
        kill -TERM "$server_pid"
        wait "$server_pid"
        trap - EXIT
      ' \
      sh "/cli/$binary" "$version"
  done
done
