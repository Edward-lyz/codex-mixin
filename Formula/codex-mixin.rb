class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "1.0.0"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.0/codex-mixin-cli-1.0.0-aarch64-apple-darwin.tar.gz"
      sha256 "a9b3095a1989af34b38e44a378f533e859eeabedffd6cacf08f10608fedce31d"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.0/codex-mixin-cli-1.0.0-x86_64-apple-darwin.tar.gz"
      sha256 "f11561516ce04073303d518e8e30d6ee9f14e96d23db0ef449cc9d930c6d03bb"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.0/codex-mixin-cli-1.0.0-aarch64-unknown-linux-musl.tar.gz"
      sha256 "f9c2a345cb7baa11b2dd550c2b0346cab6433b9308943df17b1967d3e3c5d301"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v1.0.0/codex-mixin-cli-1.0.0-x86_64-unknown-linux-musl.tar.gz"
      sha256 "1396e5ab952e449cd3ad840869438d86abc2070fcc41ea39cb4541d509016e3d"
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
