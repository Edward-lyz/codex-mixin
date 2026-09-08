#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUPPORT_FILE="$ROOT_DIR/macos/LiquidGlassSupport.swift"

require_text() {
  local file="$1"
  local text="$2"
  if ! grep -Fq "$text" "$file"; then
    echo "missing Liquid Glass contract in ${file#"$ROOT_DIR/"}: $text" >&2
    exit 1
  fi
}

require_text "$SUPPORT_FILE" "func liquidGlassProminentButton() -> some View"
require_text "$SUPPORT_FILE" "content.buttonStyle(.glassProminent)"
require_text "$SUPPORT_FILE" "func configureOpaqueWindow(_ window: NSWindow)"
require_text "$SUPPORT_FILE" "window.isOpaque = true"

for file in \
  macos/ProviderSettingsWindow.swift \
  macos/ModelBenchmarkWindow.swift \
  macos/FusionSettingsWindow.swift \
  macos/SettingsPanel.swift \
  macos/InstallCodexPanel.swift \
  macos/AboutWindow.swift \
  macos/InstallProgressWindow.swift \
  macos/InstallCard.swift \
  macos/AppSupport.swift
do
  require_text "$ROOT_DIR/$file" "configureOpaqueWindow("
done

if grep -R -Fq --include='*.swift' --exclude='LiquidGlassSupport.swift' \
  '.liquidGlass(' "$ROOT_DIR/macos"
then
  echo "content Liquid Glass effect remains outside LiquidGlassSupport.swift" >&2
  exit 1
fi

if grep -R -Fq --include='*.swift' \
  'configureLiquidGlassWindow(' "$ROOT_DIR/macos"
then
  echo "legacy transparent window helper remains" >&2
  exit 1
fi

if [ "$(grep -Fc '.textFieldStyle(.roundedBorder)' "$ROOT_DIR/macos/ModelBenchmarkWindow.swift")" -ne 2 ]; then
  echo "benchmark must retain two rounded-border input fields" >&2
  exit 1
fi

if grep -R -Fq --include='*.swift' --exclude='LiquidGlassSupport.swift' \
  '.background(.ultraThinMaterial)' "$ROOT_DIR/macos"
then
  echo "legacy ultraThinMaterial background remains outside LiquidGlassSupport.swift" >&2
  exit 1
fi

TEST_BINARY="$(mktemp -d)/liquid-glass-style-tests"
xcrun swiftc \
  "$ROOT_DIR/macos/LiquidGlassSupport.swift" \
  "$ROOT_DIR/macos/tests/LiquidGlassStyleTests.swift" \
  -framework Cocoa \
  -framework SwiftUI \
  -o "$TEST_BINARY"
"$TEST_BINARY"

echo "Liquid Glass style contract passed"
