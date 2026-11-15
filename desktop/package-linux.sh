#!/usr/bin/env bash
set -euo pipefail
if [[ $# -ne 3 ]]; then
  echo 'usage: package-linux.sh <target-triple> <version> <out-dir>' >&2
  exit 2
fi
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$1"
VERSION="$2"
OUT_DIR="$3"
case "$TARGET" in
  x86_64-unknown-linux-musl) RID=linux-x64 ;;
  aarch64-unknown-linux-musl) RID=linux-arm64 ;;
  *) echo "unsupported Linux target: $TARGET" >&2; exit 2 ;;
esac
CLI_PATH="$ROOT_DIR/target/$TARGET/release/codex-mixin"
[[ -x "$CLI_PATH" ]] || { echo "Rust CLI is missing: $CLI_PATH" >&2; exit 1; }
PUBLISH_DIR="$ROOT_DIR/desktop/build/$RID"
dotnet publish "$ROOT_DIR/desktop/CodexMixin/CodexMixin.csproj" -c Release -r "$RID" \
  --self-contained true -p:Version="$VERSION" -p:DebugType=None -p:DebugSymbols=false -o "$PUBLISH_DIR"
cp "$CLI_PATH" "$PUBLISH_DIR/codex-mixin"
cp "$ROOT_DIR/LICENSE" "$ROOT_DIR/NOTICE" "$PUBLISH_DIR/"
cp "$ROOT_DIR/desktop/codex-mixin.desktop" "$PUBLISH_DIR/"
mkdir -p "$OUT_DIR"
tar -czf "$OUT_DIR/codex-mixin-desktop-$VERSION-$RID.tar.gz" -C "$PUBLISH_DIR" .
