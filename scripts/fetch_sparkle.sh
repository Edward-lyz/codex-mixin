#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/.sparkle-version")"
EXPECTED_SHA256="$(tr -d '[:space:]' < "$ROOT_DIR/.sparkle-sha256")"
CACHE_DIR="$ROOT_DIR/.build/sparkle/$VERSION"
ARCHIVE="$CACHE_DIR/Sparkle-$VERSION.tar.xz"
INSTALL_DIR="$CACHE_DIR/tool"
URL="https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz"

verify_archive() {
  [[ -f "$ARCHIVE" ]] || return 1
  local actual_sha256
  actual_sha256="$(shasum -a 256 "$ARCHIVE" | awk '{ print $1 }')"
  [[ "$actual_sha256" == "$EXPECTED_SHA256" ]]
}

mkdir -p "$CACHE_DIR"
if ! verify_archive; then
  rm -f "$ARCHIVE"
  curl \
    --fail \
    --location \
    --silent \
    --show-error \
    --retry 3 \
    "$URL" \
    -o "$ARCHIVE.download"
  mv "$ARCHIVE.download" "$ARCHIVE"
fi

if ! verify_archive; then
  echo "Sparkle archive checksum mismatch for version $VERSION" >&2
  exit 1
fi

if [[ ! -d "$INSTALL_DIR/Sparkle.framework" || ! -x "$INSTALL_DIR/bin/generate_appcast" ]]; then
  EXTRACT_DIR="$(mktemp -d "$CACHE_DIR/extract.XXXXXX")"
  trap 'rm -rf "$EXTRACT_DIR"' EXIT
  tar -xJf "$ARCHIVE" -C "$EXTRACT_DIR"
  rm -rf "$INSTALL_DIR"
  mv "$EXTRACT_DIR" "$INSTALL_DIR"
  trap - EXIT
fi

INSTALLED_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INSTALL_DIR/Sparkle.framework/Resources/Info.plist")"
if [[ "$INSTALLED_VERSION" != "$VERSION" ]]; then
  echo "Sparkle version mismatch: expected $VERSION, got ${INSTALLED_VERSION:-missing}" >&2
  exit 1
fi

echo "$INSTALL_DIR"
