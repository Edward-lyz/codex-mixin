#!/bin/sh
# Installs the Codex Mixin CLI from GitHub Releases on Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/Edward-lyz/codex-mixin/main/scripts/install.sh | sh
#
# Environment:
#   CODEX_MIXIN_VERSION      release to install, e.g. 0.7.5 (default: latest)
#   CODEX_MIXIN_INSTALL_DIR  target directory (default: $HOME/.local/bin)
#
# POSIX sh so it runs under dash, busybox and bash alike.
set -eu

REPO_URL="https://github.com/Edward-lyz/codex-mixin"
INSTALL_DIR="${CODEX_MIXIN_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
  echo "codex-mixin install: $*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "unsupported OS $(uname -s); download a release from $REPO_URL/releases" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) fail "unsupported architecture $(uname -m)" ;;
esac
target="$arch-$os"

version="${CODEX_MIXIN_VERSION:-}"
if [ -z "$version" ]; then
  # The latest-release page redirects to .../tag/vX.Y.Z; reading the redirect
  # avoids the rate-limited GitHub API.
  latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO_URL/releases/latest")" \
    || fail "could not resolve the latest release"
  version="${latest##*/v}"
fi
version="${version#v}"
case "$version" in
  "" | *[!0-9A-Za-z.-]*) fail "invalid version '$version'" ;;
esac

archive="codex-mixin-cli-$version-$target.tar.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

echo "Downloading Codex Mixin $version ($target)"
curl -fL --progress-bar -o "$work/$archive" "$REPO_URL/releases/download/v$version/$archive" \
  || fail "download failed: $REPO_URL/releases/download/v$version/$archive"

# Verify the archive against the release's SHA256SUMS. Releases published before
# checksums existed have no such file; warn and continue for those.
if curl -fsSL -o "$work/SHA256SUMS" "$REPO_URL/releases/download/v$version/SHA256SUMS" 2>/dev/null; then
  expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1; exit }' "$work/SHA256SUMS")"
  [ -n "$expected" ] || fail "SHA256SUMS has no entry for $archive"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$work/$archive" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$work/$archive" | awk '{ print $1 }')"
  else
    fail "sha256sum or shasum is required to verify the download"
  fi
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $archive (expected $expected, got $actual); aborting"
  echo "Checksum verified"
else
  echo "Warning: this release has no SHA256SUMS; skipping checksum verification" >&2
fi

tar -xzf "$work/$archive" -C "$work"
binary="$work/codex-mixin-cli-$version-$target/codex-mixin"
[ -f "$binary" ] || fail "archive has no codex-mixin executable"

mkdir -p "$INSTALL_DIR"
# Write next to the target and rename so a running gateway keeps its inode.
cp "$binary" "$INSTALL_DIR/.codex-mixin.new"
chmod 755 "$INSTALL_DIR/.codex-mixin.new"
mv -f "$INSTALL_DIR/.codex-mixin.new" "$INSTALL_DIR/codex-mixin"

echo "Installed $("$INSTALL_DIR/codex-mixin" --version) to $INSTALL_DIR/codex-mixin"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "$INSTALL_DIR is not on PATH. Add this line to your shell profile:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
echo
echo "Next: run 'codex-mixin panel' and open the printed address in a browser."
