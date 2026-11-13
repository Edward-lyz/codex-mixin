cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.2"
  sha256 arm:   "74b3c2b4df50d25e1dbb70c8534876624d0e8241c6b8ca26bdd3a523cd00e1c9",
         intel: "9df72b191fc69533af6e405197852cdefd5e0df210b506141d3aab1a449da521"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.2/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
