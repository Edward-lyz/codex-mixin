cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.6"
  sha256 arm:   "9d9690a6bbcd411d21d1ab6a1755c2205baaf2ad812e603c9dd9817f7e047851",
         intel: "290da1523089ef30c4813e4df8dc13b265e479a5b84d50ff07e92afd63d137d6"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.6/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
