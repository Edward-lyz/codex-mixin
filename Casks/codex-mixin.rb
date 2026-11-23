cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "1.0.0"
  sha256 arm:   "a9320e01be62645ea8d2d81aeae79836c98f37959c85fe4a85fc823365878d00",
         intel: "129b44266b16e3dedecd8db05459ee2d363a85758eff6b7a08bdaf51ca11ce2f"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.0/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
