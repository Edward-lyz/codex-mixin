#!/usr/bin/env bash
# Checks desktop/locales/app.yml: every key has en and zh-CN text, and every
# key the desktop sources translate is defined.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCALES="$ROOT_DIR/desktop/locales/app.yml"
SOURCES="$ROOT_DIR/desktop/src"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

awk '
  /^[a-z][a-z0-9_.]*:[[:space:]]*$/ { key = substr($0, 1, index($0, ":") - 1); keys[key] = 1; next }
  /^  en:/ { en[key] = 1 }
  /^  zh-CN:/ { zh[key] = 1 }
  END {
    for (key in keys) {
      if (!(key in en)) print key " is missing en" > "/dev/stderr"
      if (!(key in zh)) print key " is missing zh-CN" > "/dev/stderr"
      if (!(key in en) || !(key in zh)) failed = 1
      print key
    }
    exit failed
  }
' "$LOCALES" | LC_ALL=C sort > "$TEMP_DIR/defined.keys"

DUPLICATES="$(grep -E '^[a-z][a-z0-9_.]*:[[:space:]]*$' "$LOCALES" | LC_ALL=C sort | uniq -d)"
if [[ -n "$DUPLICATES" ]]; then
  echo "duplicate localization keys: $DUPLICATES" >&2
  exit 1
fi

# Multi-line t!/tr! calls put the key on the next line, so join lines first.
find "$SOURCES" -name '*.rs' -print0 \
  | xargs -0 cat \
  | tr '\n' ' ' \
  | grep -oE '(t|tr)!\([[:space:]]*"[a-z][a-z0-9_.]*"' \
  | sed -E 's/.*"([^"]+)"/\1/' \
  | LC_ALL=C sort -u > "$TEMP_DIR/used.keys"
find "$SOURCES" -name '*.rs' -print0 \
  | xargs -0 grep -ohE '"cli\.[a-z_]+"' \
  | tr -d '"' \
  | LC_ALL=C sort -u >> "$TEMP_DIR/used.keys"
LC_ALL=C sort -u -o "$TEMP_DIR/used.keys" "$TEMP_DIR/used.keys"

if ! comm -23 "$TEMP_DIR/used.keys" "$TEMP_DIR/defined.keys" | diff -u /dev/null -; then
  echo "desktop sources use localization keys missing from app.yml" >&2
  exit 1
fi

echo "Localization keys: matched ($(wc -l < "$TEMP_DIR/defined.keys" | tr -d '[:space:]'))"
