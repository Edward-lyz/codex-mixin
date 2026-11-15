#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: package-linux-deb.sh <target-triple> <deb-arch> <version> <out-dir>" >&2
  exit 2
fi

TARGET="$1"
DEB_ARCH="$2"
VERSION="$3"
OUT_DIR="$4"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/target/$TARGET/release/codex-mixin"
PACKAGE_ROOT="$ROOT_DIR/target/package/codex-mixin-deb-$TARGET"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "missing binary: $BIN_PATH" >&2
  exit 1
fi

rm -rf "$PACKAGE_ROOT"
mkdir -p "$PACKAGE_ROOT/DEBIAN" "$PACKAGE_ROOT/usr/local/bin" "$PACKAGE_ROOT/usr/share/doc/codex-mixin"
cp "$BIN_PATH" "$PACKAGE_ROOT/usr/local/bin/codex-mixin"
chmod 0755 "$PACKAGE_ROOT/usr/local/bin/codex-mixin"
cp "$ROOT_DIR/LICENSE" "$PACKAGE_ROOT/usr/share/doc/codex-mixin/LICENSE"
cp "$ROOT_DIR/NOTICE" "$PACKAGE_ROOT/usr/share/doc/codex-mixin/NOTICE"
case "$DEB_ARCH" in
  amd64) DESKTOP_RID=linux-x64 ;;
  arm64) DESKTOP_RID=linux-arm64 ;;
  *) echo "unsupported desktop architecture: $DEB_ARCH" >&2; exit 2 ;;
esac
DESKTOP_DIR="$ROOT_DIR/desktop/build/$DESKTOP_RID"
[[ -x "$DESKTOP_DIR/codex_mixin_ui" ]] || { echo "missing desktop publish: $DESKTOP_DIR" >&2; exit 1; }
mkdir -p "$PACKAGE_ROOT/opt/codex-mixin" "$PACKAGE_ROOT/usr/share/applications" \
  "$PACKAGE_ROOT/usr/share/icons/hicolor/256x256/apps"
cp -R "$DESKTOP_DIR/." "$PACKAGE_ROOT/opt/codex-mixin/"
cp "$ROOT_DIR/desktop/codex-mixin.desktop" "$PACKAGE_ROOT/usr/share/applications/"
cp "$ROOT_DIR/panel/assets/logo.png" "$PACKAGE_ROOT/usr/share/icons/hicolor/256x256/apps/codex-mixin.png"

cat >"$PACKAGE_ROOT/DEBIAN/control" <<CONTROL
Package: codex-mixin
Version: $VERSION
Section: devel
Priority: optional
Architecture: $DEB_ARCH
Maintainer: Codex Mixin Maintainers
Depends: ca-certificates, libc6, libgcc-s1, libstdc++6, libicu-dev, libfontconfig1, libx11-6, libice6, libsm6, xdg-utils
Description: Local gateway for adding custom model providers to Codex
 Codex Mixin preserves Codex's official OpenAI account path while exposing
 custom model providers through a local Responses-compatible gateway.
CONTROL

mkdir -p "$OUT_DIR"
dpkg-deb --build "$PACKAGE_ROOT" "$OUT_DIR/codex-mixin-$VERSION-$TARGET.deb"
