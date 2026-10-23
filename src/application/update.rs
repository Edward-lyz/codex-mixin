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
    let cache_path = crate::config::stored_config_path().with_file_name("update-check.json");
    let cached = match tokio::fs::read(&cache_path).await {
        Ok(bytes) => Some(serde_json::from_slice::<ReleaseCheck>(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read release check cache"),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let release =
        check_release(&client, &format!("{RELEASES_URL}/latest"), cached.as_ref()).await?;
    let version = release.version.clone();
    let bytes = serde_json::to_vec(&release)?;
    tokio::task::spawn_blocking(move || {
        crate::platform::write_atomic_if_changed(&cache_path, &bytes)
    })
    .await
    .context("save release check worker")??;
    Ok(version)
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ReleaseCheck {
    version: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

async fn check_release(
    client: &reqwest::Client,
    url: &str,
    cached: Option<&ReleaseCheck>,
) -> anyhow::Result<ReleaseCheck> {
    // HEAD follows the release redirect without downloading the release page.
    let mut request = client.head(url);
    if let Some(cached) = cached {
        ensure_version_chars(&cached.version)?;
        if let Some(etag) = &cached.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &cached.last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
        }
    }
    let response = request
        .send()
        .await
        .context("query GitHub releases")?
        .error_for_status()?;
    let version = release_version_from_redirect(response.url().as_str())?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        let cached = cached.context("release returned 304 without cached metadata")?;
        anyhow::ensure!(
            cached.version == version,
            "release redirect changed on a 304 response"
        );
        return Ok(ReleaseCheck {
            version,
            etag: cached.etag.clone(),
            last_modified: cached.last_modified.clone(),
        });
    }
    anyhow::ensure!(
        response.status() == reqwest::StatusCode::OK,
        "unexpected release status {}",
        response.status()
    );
    Ok(ReleaseCheck {
        version,
        etag: response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        last_modified: response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    })
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
            .join(crate::platform::executable_file_name("codex-mixin"));
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

    #[tokio::test]
    async fn release_checks_use_conditional_head_after_redirect() {
        use axum::{
            Router,
            http::{HeaderMap, StatusCode},
            response::Redirect,
            routing::head,
        };
        let router = Router::new()
            .route(
                "/releases/latest",
                head(|| async { Redirect::temporary("/releases/tag/v1.2.3") }),
            )
            .route(
                "/releases/tag/v1.2.3",
                head(|headers: HeaderMap| async move {
                    let unchanged = headers
                        .get(reqwest::header::IF_NONE_MATCH)
                        .is_some_and(|value| value == "\"release-1\"");
                    if unchanged {
                        assert_eq!(
                            headers[reqwest::header::IF_MODIFIED_SINCE],
                            "Wed, 21 Oct 2026 00:00:00 GMT"
                        );
                    }
                    (
                        if unchanged {
                            StatusCode::NOT_MODIFIED
                        } else {
                            StatusCode::OK
                        },
                        [
                            (reqwest::header::ETAG, "\"release-1\""),
                            (
                                reqwest::header::LAST_MODIFIED,
                                "Wed, 21 Oct 2026 00:00:00 GMT",
                            ),
                        ],
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let first = check_release(&client, &url, None).await.unwrap();
        assert_eq!(first.version, "1.2.3");
        assert_eq!(first.etag.as_deref(), Some("\"release-1\""));
        let unchanged = check_release(&client, &url, Some(&first)).await.unwrap();
        assert_eq!(unchanged.version, first.version);
        assert_eq!(unchanged.etag, first.etag);
        server.abort();
    }
}
