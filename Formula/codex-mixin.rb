class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.7"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.7/codex-mixin-cli-0.8.7-aarch64-apple-darwin.tar.gz"
      sha256 "740d5e621faba445b078d2f51e30488e903a7aeb083994944ff1f5a6250d9f99"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.7/codex-mixin-cli-0.8.7-x86_64-apple-darwin.tar.gz"
      sha256 "0d611aa3ee80558fed5c536324f439091cd668585b6c7f10e67361e175a8caf4"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.7/codex-mixin-cli-0.8.7-aarch64-unknown-linux-musl.tar.gz"
      sha256 "b25db5c31219bf7a750a3fa32ae62202b46dbb6354a179626e236a190c0f9368"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.7/codex-mixin-cli-0.8.7-x86_64-unknown-linux-musl.tar.gz"
      sha256 "5d8a4604570f145bca187a3f0dea353df4736a01173590feb7e3f9f792d48ada"
    end
  end

  def install
    bin.install "codex-mixin"
  end

  service do
    run [opt_bin/"codex-mixin", "service", "start", "--foreground"]
    keep_alive true
    log_path var/"log/codex-mixin.log"
    error_log_path var/"log/codex-mixin.log"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/codex-mixin --version")
  end
end
