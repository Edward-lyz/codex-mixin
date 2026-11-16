cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.5"
  sha256 arm:   "cee990bb44e3fa6e5332507db1a90b345f5eb4609c58ad0c6ff00fe7557cf698",
         intel: "f4ca0f1f633124d6aa1d213797f2b39b6e6b40e8bce6e3fc604354d1b12aeae9"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.5/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
