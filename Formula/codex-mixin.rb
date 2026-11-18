class CodexMixin < Formula
  desc "Local gateway that lets Codex, Claude Code and other agents use any model"
  homepage "https://github.com/Edward-lyz/codex-mixin"
  version "0.8.6"
  license "CC-BY-NC-4.0"

  on_macos do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.6/codex-mixin-cli-0.8.6-aarch64-apple-darwin.tar.gz"
      sha256 "84a527c7c3f393721cc7fa807c3e7ef313ee409dc6ee8bf9c2537f06488fa00f"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.6/codex-mixin-cli-0.8.6-x86_64-apple-darwin.tar.gz"
      sha256 "5690abd446bc0be9ed5bfb1f2d996c20cb36140ed34a1748f5b9e8b7e2833be6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.6/codex-mixin-cli-0.8.6-aarch64-unknown-linux-musl.tar.gz"
      sha256 "d8a4726e3179c9396c937a9e77bce60b8916f6d05db6511c70713c3eb1fd0c7e"
    end
    on_intel do
      url "https://github.com/Edward-lyz/codex-mixin/releases/download/v0.8.6/codex-mixin-cli-0.8.6-x86_64-unknown-linux-musl.tar.gz"
      sha256 "91456110868138b59a41d8c8da84435e32912dd761b86f4e2289e91d6d66b911"
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
