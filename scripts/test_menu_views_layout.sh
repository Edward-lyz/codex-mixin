#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/menu-views-layout-tests"
mkdir -p "$(dirname "$TEST_BINARY")/ProviderLogos"
cp "$ROOT_DIR/macos/assets/providers/"*.svg "$(dirname "$TEST_BINARY")/ProviderLogos/"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
LOCALIZATION_BUNDLE="$($ROOT_DIR/scripts/prepare_test_localization.sh)"

xcrun swiftc \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/QuotaSupport.swift" \
  "$ROOT_DIR/macos/MenuViews.swift" \
  "$ROOT_DIR/macos/tests/MenuViewsLayoutTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
CODEX_MIXIN_LOCALIZATION_DIR="$LOCALIZATION_BUNDLE" "$TEST_BINARY"
