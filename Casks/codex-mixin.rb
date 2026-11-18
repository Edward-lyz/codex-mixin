cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.7"
  sha256 arm:   "575fec84f96abfd8143cbbb25ad085460b04d43fe2dfaf63e9e0ff39cc67981b",
         intel: "688ad1f02df29aeec38082b096baa1815b4a4bfec6f93d716c47d32addb6f1fc"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.7/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
