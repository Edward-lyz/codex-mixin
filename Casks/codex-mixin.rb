cask "codex-mixin" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.8.1"
  sha256 arm:   "a9f56ff175ce2461af869ed54ed73b938df0826cebd25873b9b66f237613073a",
         intel: "000d54f786090746ea76390197679527af47e87201bfbcfb7975de116d21879f"

  url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.1/codex-mixin-#{version}-#{arch}-apple-darwin.dmg"
  name "Codex Mixin"
  desc "Menu bar app for the Codex Mixin model gateway"
  homepage "https://github.com/Edward-lyz/codex-mixin"

  auto_updates true
  depends_on macos: ">= :ventura"
  conflicts_with formula: "codex-mixin"

  app "Codex Mixin.app"

  zap trash: "~/.codex-mixin"
end
