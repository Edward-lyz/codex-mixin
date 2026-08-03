#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/.swiftgen-version")"
CACHE_DIR="$ROOT_DIR/.build/swiftgen/$VERSION"
INSTALL_DIR="$CACHE_DIR/tool"
BINARY="$INSTALL_DIR/bin/swiftgen"
ARCHIVE="$CACHE_DIR/swiftgen-$VERSION.zip"
URL="https://github.com/SwiftGen/SwiftGen/releases/download/$VERSION/swiftgen-$VERSION.zip"

if [[ ! -x "$BINARY" || ! -d "$INSTALL_DIR/bin/SwiftGen_SwiftGenCLI.bundle" ]]; then
  mkdir -p "$CACHE_DIR"
  if [[ ! -f "$ARCHIVE" ]]; then
    curl --fail --location --silent --show-error --retry 3 "$URL" -o "$ARCHIVE"
  fi
  rm -rf "$INSTALL_DIR"
  mkdir -p "$INSTALL_DIR"
  unzip -q "$ARCHIVE" -d "$INSTALL_DIR"
fi

actual_version="$($BINARY --version | sed -n '1s/^SwiftGen v\([^ ]*\).*$/\1/p')"
if [[ "$actual_version" != "$VERSION" ]]; then
  echo "SwiftGen version mismatch: expected $VERSION, got $actual_version" >&2
  exit 1
fi

mkdir -p "$ROOT_DIR/macos/Generated"
exec "$BINARY" "$@"
