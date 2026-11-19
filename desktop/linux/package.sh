#!/usr/bin/env bash
# Builds the Linux desktop package: the GPUI shell (glibc, built for the host)
# with the static CLI beside it, where the shell looks for codex-mixin. The
# tarball holds one top-level folder, which is both what users unpack and
# what the in-app updater swaps in.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: package.sh <cli-target-triple> <version> <out-dir>" >&2
  exit 2
fi

CLI_TARGET="$1"
VERSION="$2"
OUT_DIR="$3"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLI_PATH="$ROOT_DIR/target/$CLI_TARGET/release/codex-mixin"

if [[ ! -x "$CLI_PATH" ]]; then
  echo "missing CLI binary: $CLI_PATH" >&2
  exit 1
fi

cargo build --release --manifest-path "$ROOT_DIR/desktop/Cargo.toml"
APP_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
PACKAGE_ROOT="$ROOT_DIR/target/package/codex-mixin-app-$APP_TARGET"
STAGING="$PACKAGE_ROOT/codex-mixin"

rm -rf "$PACKAGE_ROOT"
mkdir -p "$STAGING" "$OUT_DIR"
install -m 0755 "$ROOT_DIR/desktop/target/release/codex-mixin-desktop" "$STAGING/codex-mixin-desktop"
install -m 0755 "$CLI_PATH" "$STAGING/codex-mixin"
install -m 0755 "$ROOT_DIR/desktop/linux/install.sh" "$STAGING/install.sh"
install -m 0644 "$ROOT_DIR/desktop/assets/app/app-icon.png" "$STAGING/codex-mixin.png"
install -m 0644 "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$ROOT_DIR/NOTICE" "$STAGING/"

ARCHIVE="$OUT_DIR/codex-mixin-app-$VERSION-$APP_TARGET.tar.gz"
tar -czf "$ARCHIVE" -C "$PACKAGE_ROOT" codex-mixin
echo "Built $ARCHIVE"
