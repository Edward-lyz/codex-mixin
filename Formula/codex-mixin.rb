class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.2"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.2/codex-mixin-cli-0.8.2-aarch64-apple-darwin.tar.gz"
      sha256 "6e5048c83fe8f55049d155ec80b8264feea9d5f561217385ede1776b85f5ff0e"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.2/codex-mixin-cli-0.8.2-x86_64-apple-darwin.tar.gz"
      sha256 "7cfa068881e8937dd46bd77f63d2e62bf6768ac2a648b12232f0ae4cffcb7bc8"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.2/codex-mixin-cli-0.8.2-aarch64-unknown-linux-musl.tar.gz"
      sha256 "3cae6d7aa6f1be4fa2d3c5fcb43a7bab25b831b67c4b8f62a047097759ede005"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.2/codex-mixin-cli-0.8.2-x86_64-unknown-linux-musl.tar.gz"
      sha256 "606954d626d66303ea6a661d5401086336bd1448cff1ce8d3a2f1a633dbdc018"
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
