#!/bin/sh
# Served by the application deployment, independently of the download bucket.
set -eu

base_url='https://magicmtn-cli-993499103295.s3.us-east-2.amazonaws.com'
# The application embeds these public values from committed release/cli-trust.json.
apple_team_id='@APPLE_TEAM_ID@'
public_key=$(cat <<'MAGICMTN_PUBLIC_KEY'
@RELEASE_PUBLIC_KEY@
MAGICMTN_PUBLIC_KEY
)

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

for tool in curl openssl; do
  command -v "$tool" >/dev/null 2>&1 || { printf 'Error: %s is required to verify the CLI release.\n' "$tool" >&2; exit 1; }
done
printf '%s\n' "$public_key" > "$temporary/public.pem"
printf 'Verifying MagicMtn release…\n'
curl -fsSL "$base_url/latest.signed.json" -o "$temporary/envelope.json"
# The publisher writes this canonical, single-line envelope. Both fields contain
# only base64, so parsing does not require executing downloaded code or jq/Python.
test "$(wc -l < "$temporary/envelope.json" | tr -d '[:space:]')" = 1
grep -Eq '^\{"payload":"[A-Za-z0-9+/=]+","signature":"[A-Za-z0-9+/=]+"\}$' "$temporary/envelope.json" || {
  printf 'Error: a signed release manifest is required.\n' >&2; exit 1;
}
sed -n 's/^{"payload":"\([A-Za-z0-9+/=]*\)","signature":"[A-Za-z0-9+/=]*"}$/\1/p' "$temporary/envelope.json" | openssl base64 -d -A > "$temporary/payload.json"
sed -n 's/^{"payload":"[A-Za-z0-9+/=]*","signature":"\([A-Za-z0-9+/=]*\)"}$/\1/p' "$temporary/envelope.json" | openssl base64 -d -A > "$temporary/signature"
if ! openssl dgst -sha256 -verify "$temporary/public.pem" -signature "$temporary/signature" "$temporary/payload.json" >/dev/null 2>&1; then
  printf 'Error: the release signature is invalid; nothing was installed.\n' >&2; exit 1;
fi
release=$(sed -n 's/.*"release":"\([A-Za-z0-9.-]*\)".*/\1/p' "$temporary/payload.json")
expected=$(sed -n 's/.*"'"$platform"'":"\([0-9a-f]\{64\}\)".*/\1/p' "$temporary/payload.json")
issued=$(sed -n 's/.*"issued_at":\([0-9][0-9]*\).*/\1/p' "$temporary/payload.json")
expires=$(sed -n 's/.*"expires_at":\([0-9][0-9]*\).*/\1/p' "$temporary/payload.json")
channel=$(sed -n 's/.*"channel":"\([a-z]*\)".*/\1/p' "$temporary/payload.json")
team=$(sed -n 's/.*"apple_team_id":"\([A-Z0-9]*\)".*/\1/p' "$temporary/payload.json")
grep -Eq '"schema":1([,}])' "$temporary/payload.json"
test -n "$release" && test "${#release}" -le 100 && test "${#expected}" = 64
test "$team" = "$apple_team_id"
case "$channel" in stable) ;; *) printf 'Error: unsupported release channel.\n' >&2; exit 1 ;; esac
now=$(date -u +%s)
test -n "$issued" && test -n "$expires"
if ! { test "$issued" -le "$((now + 300))" && test "$expires" -gt "$now" && test "$expires" -gt "$issued" && test "$((expires - issued))" -le 1209600; }; then
  printf 'Error: release metadata has expired or has invalid dates.\n' >&2; exit 1;
fi
printf 'Downloading authenticated MagicMtn CLI…\n'
curl -fsSL "$base_url/releases/$release/magic-$platform" -o "$temporary/magic"
actual=$(openssl dgst -sha256 "$temporary/magic" | awk '{print $NF}')
if [ "$actual" != "$expected" ]; then
  printf 'Error: the CLI does not match its signed SHA-256; nothing was installed.\n' >&2; exit 1;
fi
if [ "$platform" = darwin-arm64 ]; then
  /usr/bin/codesign --verify --strict -R "anchor apple generic and certificate leaf[subject.OU] = \"$apple_team_id\"" "$temporary/magic"
fi
chmod 755 "$temporary/magic"
if ! "$temporary/magic" --version; then
  printf 'Error: the downloaded CLI could not run; the existing installation has not been changed.\n' >&2
  exit 1
fi
if [ -d "$install_dir/magic" ]; then
  printf 'Error: %s/magic is a directory.\n' "$install_dir" >&2; exit 1
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
