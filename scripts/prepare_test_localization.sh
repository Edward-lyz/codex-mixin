#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_BUNDLE="$(mktemp -d)/CodexMixinTests.bundle"
mkdir -p "$TEST_BUNDLE/en.lproj" "$TEST_BUNDLE/zh-Hans.lproj"
cp "$ROOT_DIR/macos/en.lproj/Localizable.strings" "$TEST_BUNDLE/en.lproj/"
cp "$ROOT_DIR/macos/zh-Hans.lproj/Localizable.strings" "$TEST_BUNDLE/zh-Hans.lproj/"
printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '<key>CFBundleIdentifier</key><string>local.codex-mixin.tests</string>' \
  '<key>CFBundlePackageType</key><string>BNDL</string>' \
  '<key>CFBundleDevelopmentRegion</key><string>en</string>' \
  '<key>CFBundleLocalizations</key><array><string>en</string><string>zh-Hans</string></array>' \
  '</dict></plist>' > "$TEST_BUNDLE/Info.plist"
printf '%s\n' "$TEST_BUNDLE"
