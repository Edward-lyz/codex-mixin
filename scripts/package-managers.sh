#!/usr/bin/env bash
# Write Homebrew formula and cask for a published release.
# usage: package-managers.sh <version> <release-dir> <out-dir>
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: package-managers.sh <version> <release-dir> <out-dir>" >&2
  exit 2
fi

VERSION="$1"
RELEASE_DIR="$2"
OUT_DIR="$3"
REPO="Edward-lyz/codex-mixin"
BASE_URL="https://github.com/$REPO/releases/download/v$VERSION"

sha() {
  local file="$RELEASE_DIR/$1"
  if [[ ! -s "$file" ]]; then
    echo "missing release asset: $1" >&2
    exit 1
  fi
  shasum -a 256 "$file" | awk '{print $1}'
}

cli_mac_arm="codex-mixin-cli-$VERSION-aarch64-apple-darwin.tar.gz"
cli_mac_x64="codex-mixin-cli-$VERSION-x86_64-apple-darwin.tar.gz"
cli_linux_arm="codex-mixin-cli-$VERSION-aarch64-unknown-linux-musl.tar.gz"
cli_linux_x64="codex-mixin-cli-$VERSION-x86_64-unknown-linux-musl.tar.gz"
dmg_arm="codex-mixin-$VERSION-aarch64-apple-darwin.dmg"
dmg_x64="codex-mixin-$VERSION-x86_64-apple-darwin.dmg"

mkdir -p "$OUT_DIR/Formula" "$OUT_DIR/Casks"

cat >"$OUT_DIR/Formula/codex-mixin.rb" <<EOF
class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/$REPO"
  version "$VERSION"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "$BASE_URL/$cli_mac_arm"
      sha256 "$(sha "$cli_mac_arm")"
    end
    on_intel do
      url "$BASE_URL/$cli_mac_x64"
      sha256 "$(sha "$cli_mac_x64")"
    end
  end

  on_linux do
    on_arm do
      url "$BASE_URL/$cli_linux_arm"
      sha256 "$(sha "$cli_linux_arm")"
    end
    on_intel do
      url "$BASE_URL/$cli_linux_x64"
      sha256 "$(sha "$cli_linux_x64")"
    end
  end

  def install
    bin.install "codex-mixin"
  end

  service do
    run [opt_bin/"codex-mixin", "service", "start", "--foreground"]
    keep_alive true
    log_path var/"log/codex-mixin.log"
    error_log_path var/"log/codex-mixin.log"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/codex-mixin --version")
  end
end
EOF

cat >"$OUT_DIR/Casks/codex-mixin.rb" <<EOF
cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "$VERSION"
  sha256 arm:   "$(sha "$dmg_arm")",
         intel: "$(sha "$dmg_x64")"

  url "$BASE_URL/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/$REPO"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
EOF

