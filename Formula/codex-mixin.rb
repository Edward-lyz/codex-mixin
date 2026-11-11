class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.0"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.0/codex-mixin-cli-0.8.0-aarch64-apple-darwin.tar.gz"
      sha256 "8c99a877e3b90e3f7f9729ee0da948ee4d5e1b44db149e650f0973bfbc633032"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.0/codex-mixin-cli-0.8.0-x86_64-apple-darwin.tar.gz"
      sha256 "57ed15022fe883b2dee1557adc1bc97829d4050e0a3a0bfac286885c6cfb51ed"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.0/codex-mixin-cli-0.8.0-aarch64-unknown-linux-musl.tar.gz"
      sha256 "b2f7f6ab0d8a9c45cf20a83d342b78a2c019cc59cb8cec3ad3ff69cbc5933768"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.0/codex-mixin-cli-0.8.0-x86_64-unknown-linux-musl.tar.gz"
      sha256 "ee128efa962566cbd1700c899a2e9289e21a967c3be5508bc53e809475c3418c"
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
