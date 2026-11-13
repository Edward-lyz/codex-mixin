class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.3"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.3/codex-mixin-cli-0.8.3-aarch64-apple-darwin.tar.gz"
      sha256 "4cccf9254f4cca3a07139e03405ba360f0ad8c1d8e977138a483bf3bef68b3af"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.3/codex-mixin-cli-0.8.3-x86_64-apple-darwin.tar.gz"
      sha256 "a2c0ff0fcd7f35156aa7bf0ffc0acb00254c82f777c9f302aee67c8a28bb7347"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.3/codex-mixin-cli-0.8.3-aarch64-unknown-linux-musl.tar.gz"
      sha256 "ba2bb3622880d06ac17f446fe10cbae521c1e29be02a65037167fbccc59c920c"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.3/codex-mixin-cli-0.8.3-x86_64-unknown-linux-musl.tar.gz"
      sha256 "23fe08565eff08719434f2b3678fc2c0362423631286b10ab5aa21016b0906ea"
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
