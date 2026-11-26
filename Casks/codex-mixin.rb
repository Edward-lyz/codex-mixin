cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "1.0.1"
  sha256 arm:   "f65239988c775f47a153a63c11fc1ede73e1d748cc6db61d7b622bc5d9da3fd9",
         intel: "0997398d5f0f4f61c6a1465ded5eb176991444fd5aa577f8847d7cd763482026"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.1/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
