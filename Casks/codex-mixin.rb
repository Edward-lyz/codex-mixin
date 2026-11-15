cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.4"
  sha256 arm:   "e878d400071d2e90b146e5faf27017c726f9919db3df49d75bb72c664fb9f0e4",
         intel: "245bd7853bb5ce1a6196a7e517b1d7e6a19dc5bc99be8169bc94e3db3482e383"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.4/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
