#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d)"
TEST_APP="$TEST_ROOT/ApplicationIconSupportTests.app"
TEST_CONTENTS="$TEST_APP/Contents"
TEST_BINARY="$TEST_CONTENTS/MacOS/ApplicationIconSupportTests"
trap 'rm -rf "$TEST_ROOT"' EXIT

mkdir -p "$TEST_CONTENTS/MacOS" "$TEST_CONTENTS/Resources"
cp "$ROOT_DIR/macos/tests/ApplicationIconSupportTests-Info.plist" "$TEST_CONTENTS/Info.plist"
cp "$ROOT_DIR/macos/CodexMixin.icns" "$TEST_CONTENTS/Resources/CodexMixin.icns"
cp "$ROOT_DIR/macos/CodexMixinDark.icns" "$TEST_CONTENTS/Resources/CodexMixinDark.icns"

xcrun swiftc \
  "$ROOT_DIR/macos/ApplicationIconSupport.swift" \
  "$ROOT_DIR/macos/tests/ApplicationIconSupportTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
