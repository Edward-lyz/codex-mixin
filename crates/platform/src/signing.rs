//! Code-signing requirements for executables the gateway rewrites.

use std::path::Path;

/// Make a byte-patched executable runnable again. macOS kills binaries whose
/// signature no longer matches their contents, so re-sign ad hoc there; other
/// systems do not verify signatures at exec time.
pub fn prepare_modified_executable(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use anyhow::Context;

        let output = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(path)
            .output()
            .context("start codesign")?;
        anyhow::ensure!(
            output.status.success(),
            "codesign failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(())
    }
}
