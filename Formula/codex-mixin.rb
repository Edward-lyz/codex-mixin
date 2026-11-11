class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.1"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.1/codex-mixin-cli-0.8.1-aarch64-apple-darwin.tar.gz"
      sha256 "e4e643bad16acb4768d687c37fd09267242d7db645c47edd44baadd7f91e254e"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.1/codex-mixin-cli-0.8.1-x86_64-apple-darwin.tar.gz"
      sha256 "28b611ced400f1856178690f8c0144e123564e481a4ae980e65c727992efbcb6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.1/codex-mixin-cli-0.8.1-aarch64-unknown-linux-musl.tar.gz"
      sha256 "b7b06455ae79f6d8ac0adbc73fd8afc85018ee841ad7adc2a406bf6b029cc2c6"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.1/codex-mixin-cli-0.8.1-x86_64-unknown-linux-musl.tar.gz"
      sha256 "7072f17a512c331ca1e541cda239a8fb41031eb9cbd4d5aaad05d7c12ce6dff3"
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
