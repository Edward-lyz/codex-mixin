class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.5"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.5/codex-mixin-cli-0.8.5-aarch64-apple-darwin.tar.gz"
      sha256 "69ff6aca1a1c7570d9abff54babefb39ac21af7a35acc6e4880f0373de815d1b"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.5/codex-mixin-cli-0.8.5-x86_64-apple-darwin.tar.gz"
      sha256 "97d32cb08679ef515ba505d76717b1016a533e852b55c4836a6c43e5366a0ad9"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.5/codex-mixin-cli-0.8.5-aarch64-unknown-linux-musl.tar.gz"
      sha256 "03026c72ac083edef4fb5ff7ad271ae1efd8dcd036f4090c44b96502d8683c78"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.5/codex-mixin-cli-0.8.5-x86_64-unknown-linux-musl.tar.gz"
      sha256 "bc7123355594fa5114adc25cebe139e298e59848fd92f992b75858bb5540c789"
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
