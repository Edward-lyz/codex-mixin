#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/application-menu-tests"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
LOCALIZATION_BUNDLE="$($ROOT_DIR/scripts/prepare_test_localization.sh)"

xcrun swiftc \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/ApplicationMenuSupport.swift" \
  "$ROOT_DIR/macos/tests/ApplicationMenuSupportTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
CODEX_MIXIN_LOCALIZATION_DIR="$LOCALIZATION_BUNDLE" "$TEST_BINARY"

if rg -n 'NSPanel\(' "$ROOT_DIR/macos" --glob '*.swift' --glob '!**/tests/**'; then
  echo "App windows must use NSWindow so they remain visible after deactivation" >&2
  exit 1
fi
if rg -n 'hidesOnDeactivate\s*=\s*true' "$ROOT_DIR/macos" --glob '*.swift' --glob '!**/tests/**'; then
  echo "App windows must not hide when another app becomes active" >&2
  exit 1
fi
if rg -n 'makeKeyAndOrderFront|NSApp\.activate' "$ROOT_DIR/macos" \
  --glob '*.swift' \
  --glob '!ApplicationMenuSupport.swift' \
  --glob '!**/tests/**'; then
  echo "App windows must use the persistent presentation coordinator" >&2
  exit 1
fi
