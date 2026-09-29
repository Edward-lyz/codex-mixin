use anyhow::Context;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const RELEASES_URL: &str = "https://github.com/Edward-lyz/codex-mixin/releases";
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

pub fn release_version_from_redirect(effective_url: &str) -> anyhow::Result<String> {
    let segment = effective_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty() && *segment != "latest")
        .ok_or_else(|| {
            anyhow::anyhow!("GitHub did not redirect to a release; proxy or rate limit response")
        })?;
    let version = segment.trim_start_matches('v');
    ensure_version_chars(version)?;
    Ok(version.to_owned())
}

fn ensure_version_chars(version: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'),
        "GitHub returned an invalid release version: {version}"
    );
    Ok(())
}

pub async fn latest_version() -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let response = client
        .get(format!("{RELEASES_URL}/latest"))
        .send()
        .await
        .context("query GitHub releases")?
        .error_for_status()?;
    release_version_from_redirect(response.url().as_str())
}

pub fn archive_name(version: &str, target: &str) -> anyhow::Result<String> {
    ensure_version_chars(version)?;
    Ok(format!("codex-mixin-cli-{version}-{target}.tar.gz"))
}

pub async fn install_version(version: &str, executable: &Path) -> anyhow::Result<()> {
    let target = crate::platform::update::release_target()?;
    let archive_name = archive_name(version, target)?;
    let temp = tempfile::tempdir()?;
    let archive = temp.path().join("release.tar.gz");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;
    let mut response = client
        .get(format!("{RELEASES_URL}/download/v{version}/{archive_name}"))
        .send()
        .await?
        .error_for_status()?;
    let mut file = tokio::fs::File::create(&archive).await?;
    let mut received = 0_u64;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        anyhow::ensure!(
            received <= MAX_ARCHIVE_BYTES,
            "CLI release archive exceeds size limit"
        );
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    let executable = executable.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::platform::update::unpack_release(&archive, temp.path())?;
        let downloaded = temp
            .path()
            .join(format!("codex-mixin{}", std::env::consts::EXE_SUFFIX));
        anyhow::ensure!(
            downloaded.is_file(),
            "release archive has no CLI executable"
        );
        anyhow::ensure!(
            std::fs::metadata(&downloaded)?.len() > 1024 * 1024,
            "downloaded CLI executable is unexpectedly small"
        );
        crate::platform::update::replace_executable(&executable, &downloaded)
    })
    .await
    .context("CLI update worker failed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_name_matches_release_packaging() {
        assert_eq!(
            archive_name("0.7.0", "x86_64-pc-windows-msvc").unwrap(),
            "codex-mixin-cli-0.7.0-x86_64-pc-windows-msvc.tar.gz"
        );
        assert!(archive_name("../invalid", "target").is_err());
    }
}
