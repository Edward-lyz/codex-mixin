use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Deserialize, Serialize)]
struct CodexAuthFile {
    #[serde(default)]
    tokens: Option<CodexAuthTokens>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CodexAuthTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CodexOfficialAuth {
    pub access_token: String,
    pub account_id: String,
}

pub async fn refresh_codex_official_auth(
    client: &reqwest::Client,
    auth_path: &std::path::Path,
    token_url: &str,
) -> anyhow::Result<CodexOfficialAuth> {
    let raw = std::fs::read_to_string(&auth_path)
        .with_context(|| format!("read Codex auth file {}", auth_path.display()))?;
    let mut auth_file: CodexAuthFile = serde_json::from_str(&raw)
        .with_context(|| format!("parse Codex auth file {}", auth_path.display()))?;
    let tokens = auth_file
        .tokens
        .clone()
        .ok_or_else(|| anyhow!("Codex auth file does not contain tokens"))?;
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow!("Codex auth file does not contain refresh_token"))?;
    let account_id = tokens
        .account_id
        .clone()
        .filter(|account_id| !account_id.is_empty())
        .ok_or_else(|| anyhow!("Codex auth file does not contain account_id"))?;

    let response = client
        .post(token_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_urlencoded(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CODEX_CLIENT_ID),
            ("scope", "openid profile email"),
        ]))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("Codex OAuth refresh returned {status}: {body}");
    }
    let refreshed: OAuthTokenResponse =
        serde_json::from_str(&body).context("parse Codex OAuth refresh response")?;
    if refreshed.access_token.is_empty() {
        anyhow::bail!("Codex OAuth refresh response does not contain access_token");
    }
    if refreshed.refresh_token.is_some() || refreshed.id_token.is_some() {
        let mut updated = tokens;
        updated.access_token = Some(refreshed.access_token.clone());
        if let Some(refresh_token) = refreshed.refresh_token {
            updated.refresh_token = Some(refresh_token);
        }
        if let Some(id_token) = refreshed.id_token {
            updated.id_token = Some(id_token);
        }
        auth_file.tokens = Some(updated);
        write_codex_auth_file(&auth_path, &auth_file)?;
    }
    Ok(CodexOfficialAuth {
        access_token: refreshed.access_token,
        account_id,
    })
}

fn write_codex_auth_file(path: &std::path::Path, auth_file: &CodexAuthFile) -> anyhow::Result<()> {
    let content = serde_json::to_vec_pretty(auth_file)?;
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn form_urlencoded(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
