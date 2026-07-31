#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/install-card-tests"
SWIFT_ARCH="$(uname -m)"

xcrun swiftc \
  -target "$SWIFT_ARCH-apple-macosx13.1" \
  "$ROOT_DIR/macos/InstallCard.swift" \
  "$ROOT_DIR/macos/tests/InstallCardTests.swift" \
  -framework Cocoa \
  -framework CryptoKit \
  -framework SwiftUI \
  -o "$TEST_BINARY"
CODEX_MIXIN_WALLPAPER_ASSET_DIR="$ROOT_DIR/macos/assets/nasa-wallpapers" "$TEST_BINARY"
