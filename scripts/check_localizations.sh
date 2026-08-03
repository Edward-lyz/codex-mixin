#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENGLISH="$ROOT_DIR/macos/en.lproj/Localizable.strings"
CHINESE="$ROOT_DIR/macos/zh-Hans.lproj/Localizable.strings"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

for strings_file in "$ENGLISH" "$CHINESE"; do
  plutil -lint "$strings_file" >/dev/null
done

extract_keys() {
  plutil -p "$1" \
    | sed -n 's/^  "\(.*\)" => .*$/\1/p' \
    | LC_ALL=C sort
}

extract_keys "$ENGLISH" > "$TEMP_DIR/en.keys"
extract_keys "$CHINESE" > "$TEMP_DIR/zh-Hans.keys"

if ! diff -u "$TEMP_DIR/en.keys" "$TEMP_DIR/zh-Hans.keys"; then
  echo "Localizable.strings keys differ between en and zh-Hans" >&2
  exit 1
fi

# Prefer rg when available, but keep CI runners that only ship stock tools working.
if command -v rg >/dev/null 2>&1; then
  rg -o 'AppLocalization\.string\("[^"]+"' "$ROOT_DIR/macos" --glob '*.swift'
else
  find "$ROOT_DIR/macos" -name '*.swift' -print0 \
    | xargs -0 grep -oE 'AppLocalization\.string\("[^"]+"' || true
fi \
  | sed -E 's/.*AppLocalization\.string\("([^"]+)"/\1/' \
  | LC_ALL=C sort -u > "$TEMP_DIR/used.keys"
if ! comm -23 "$TEMP_DIR/used.keys" "$TEMP_DIR/en.keys" | diff -u /dev/null -; then
  echo "Swift code references localization keys missing from en.lproj" >&2
  exit 1
fi

echo "Localization keys: matched ($(wc -l < "$TEMP_DIR/en.keys" | tr -d '[:space:]'))"
