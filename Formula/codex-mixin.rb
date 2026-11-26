class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "1.0.1"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.1/codex-mixin-cli-1.0.1-aarch64-apple-darwin.tar.gz"
      sha256 "e728adc36f8fe9f6465ce4df0fd0cd2fdcb41dc2b84b86096521161de2c23a41"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.1/codex-mixin-cli-1.0.1-x86_64-apple-darwin.tar.gz"
      sha256 "1440658b6af23482db21a28fbfdec6483ee1a727d6a23f9340b957a51926a98c"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.1/codex-mixin-cli-1.0.1-aarch64-unknown-linux-musl.tar.gz"
      sha256 "4bf6432c8304829d56be377b4f56f74423b83c6f6338b4058364c207ed897854"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.1/codex-mixin-cli-1.0.1-x86_64-unknown-linux-musl.tar.gz"
      sha256 "0f08c8c06e34091c9072e9982018a45247527e6a44b1250c25b41bc616eefa64"
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
