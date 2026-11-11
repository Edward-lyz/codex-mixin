#!/usr/bin/env bash
# Write Homebrew and winget manifests for a published release.
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
setup="codex-mixin-$VERSION-windows-x86_64-setup.exe"

mkdir -p "$OUT_DIR/Formula" "$OUT_DIR/Casks" "$OUT_DIR/winget"

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

# winget multi-file manifest (schema 1.6) for the NSIS installer.
id="EdwardLyz.CodexMixin"
winget_dir="$OUT_DIR/winget/manifests/e/EdwardLyz/CodexMixin/$VERSION"
mkdir -p "$winget_dir"
cat >"$winget_dir/$id.yaml" <<EOF
PackageIdentifier: $id
PackageVersion: $VERSION
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
EOF
cat >"$winget_dir/$id.installer.yaml" <<EOF
PackageIdentifier: $id
PackageVersion: $VERSION
InstallerType: nullsoft
Scope: machine
ElevationRequirement: elevatesSelf
UpgradeBehavior: install
ProductCode: CodexMixin
Installers:
  - Architecture: x64
    InstallerUrl: $BASE_URL/$setup
    InstallerSha256: $(sha "$setup" | tr '[:lower:]' '[:upper:]')
ManifestType: installer
ManifestVersion: 1.6.0
EOF
cat >"$winget_dir/$id.locale.en-US.yaml" <<EOF
PackageIdentifier: $id
PackageVersion: $VERSION
PackageLocale: en-US
Publisher: Edward-lyz
PublisherUrl: https://github.com/Edward-lyz
PackageName: Codex Mixin
PackageUrl: https://github.com/$REPO
License: CC-BY-NC-4.0
LicenseUrl: https://github.com/$REPO/blob/main/LICENSE
ShortDescription: Local gateway that lets Codex, Claude Code and other agents use any model.
ReleaseNotesUrl: https://github.com/$REPO/releases/tag/v$VERSION
ManifestType: defaultLocale
ManifestVersion: 1.6.0
EOF
