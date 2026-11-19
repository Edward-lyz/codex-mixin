#!/usr/bin/env bash
# Installs Codex Mixin for the current user: the app folder goes to
# ~/.local/opt/codex-mixin, where the in-app updater can replace it without
# root, with a launcher entry and the CLI on ~/.local/bin.
# User configuration in ~/.codex-mixin is never touched.
set -euo pipefail

SOURCE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
DESTINATION="$HOME/.local/opt/codex-mixin"
BIN_DIR="$HOME/.local/bin"

if [[ "$SOURCE" != "$DESTINATION" ]]; then
  rm -rf "$DESTINATION.new"
  mkdir -p "$(dirname "$DESTINATION")"
  cp -R "$SOURCE" "$DESTINATION.new"
  rm -rf "$DESTINATION"
  mv "$DESTINATION.new" "$DESTINATION"
fi

mkdir -p "$BIN_DIR" "$DATA_HOME/applications"
ln -sf "$DESTINATION/codex-mixin" "$BIN_DIR/codex-mixin"
# Wayland matches the window app id to the desktop file name.
cat >"$DATA_HOME/applications/com.edward.codex-mixin.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Codex Mixin
Comment=Custom model providers for Codex
Exec="$DESTINATION/codex-mixin-desktop"
Icon=$DESTINATION/codex-mixin.png
Terminal=false
Categories=Development;
StartupWMClass=com.edward.codex-mixin
DESKTOP

echo "Installed Codex Mixin to $DESTINATION"
