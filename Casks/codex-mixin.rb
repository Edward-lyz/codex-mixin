cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.3"
  sha256 arm:   "16592c0b304cab24db5090d2ae7c6af20b8ed778762614398f27e1be81c599a6",
         intel: "13723fdb92ea3544c3aaae743d0b814b0a754dd1e2e61382b0082678c9299e5c"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.3/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
