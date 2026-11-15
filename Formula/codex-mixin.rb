class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.4"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.4/codex-mixin-cli-0.8.4-aarch64-apple-darwin.tar.gz"
      sha256 "1235d19632f7526aaae67dc6767bed9af094cbff6508a1912c1c6c221a9932ba"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.4/codex-mixin-cli-0.8.4-x86_64-apple-darwin.tar.gz"
      sha256 "58fbc54284dd550571191f5cbb49c6c502bb6f45865e096c3bbd5e2c3f7d0bec"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.4/codex-mixin-cli-0.8.4-aarch64-unknown-linux-musl.tar.gz"
      sha256 "2c164884295cdda5b05b7c647a82eb150e685634de9365239e62a537bbf65125"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.4/codex-mixin-cli-0.8.4-x86_64-unknown-linux-musl.tar.gz"
      sha256 "2d72b7b83226f09eea8bc530aa0cf0837a08c197d00145945542edb4bab80e56"
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
