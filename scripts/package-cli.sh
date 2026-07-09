#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: package-cli.sh <target-triple> <version> <out-dir>" >&2
  exit 2
fi

TARGET="$1"
VERSION="$2"
OUT_DIR="$3"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGING_DIR="$ROOT_DIR/target/package/codex-mixin-cli-$VERSION-$TARGET"
BIN_PATH="$ROOT_DIR/target/$TARGET/release/codex-mixin"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "missing binary: $BIN_PATH" >&2
  exit 1
fi

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
cp "$BIN_PATH" "$STAGING_DIR/codex-mixin"
cp "$ROOT_DIR/README.md" "$STAGING_DIR/README.md"

mkdir -p "$OUT_DIR"
tar -C "$(dirname "$STAGING_DIR")" -czf \
  "$OUT_DIR/codex-mixin-cli-$VERSION-$TARGET.tar.gz" \
  "$(basename "$STAGING_DIR")"
