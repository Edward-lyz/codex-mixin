cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.0"
  sha256 arm:   "044fb47644c04b41e06bfc93baeab49efe0b35268996c116d61ae34d9e1ac55c",
         intel: "94ee344067c985a7253596eb5c4cbefad9814a09e1d1765b1f72403b220eaa8b"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.0/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
