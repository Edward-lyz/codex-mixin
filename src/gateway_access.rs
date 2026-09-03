use axum::http::{HeaderMap, header};
use base64::Engine;
use ring::rand;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

pub const CODEX_CLIENT_KEY_HEADER: &str = "x-codex-mixin-client-key";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayClient {
    Codex,
    Claude,
    Dsh,
    OpenCode,
    Pi,
}

impl GatewayClient {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Dsh => "dsh",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Dsh => "DSH",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GatewayClientKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
}

impl GatewayClientKeys {
    pub fn get(&self, client: GatewayClient) -> Option<&str> {
        match client {
            GatewayClient::Codex => self.codex.as_deref(),
            GatewayClient::Claude => self.claude.as_deref(),
            GatewayClient::Dsh => self.dsh.as_deref(),
            GatewayClient::OpenCode => self.opencode.as_deref(),
            GatewayClient::Pi => self.pi.as_deref(),
        }
    }

    pub fn get_mut(&mut self, client: GatewayClient) -> &mut Option<String> {
        match client {
            GatewayClient::Codex => &mut self.codex,
            GatewayClient::Claude => &mut self.claude,
            GatewayClient::Dsh => &mut self.dsh,
            GatewayClient::OpenCode => &mut self.opencode,
            GatewayClient::Pi => &mut self.pi,
        }
    }

    pub fn authenticate(&self, headers: &HeaderMap) -> Option<GatewayClient> {
        let codex_key = headers
            .get(CODEX_CLIENT_KEY_HEADER)
            .and_then(|value| value.to_str().ok());
        if self.matches(GatewayClient::Codex, codex_key) {
            return Some(GatewayClient::Codex);
        }
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        [
            GatewayClient::Claude,
            GatewayClient::Dsh,
            GatewayClient::OpenCode,
            GatewayClient::Pi,
        ]
        .into_iter()
        .find(|client| self.matches(*client, bearer))
    }

    fn matches(&self, client: GatewayClient, actual: Option<&str>) -> bool {
        let (Some(expected), Some(actual)) = (self.get(client), actual) else {
            return false;
        };
        bool::from(actual.as_bytes().ct_eq(expected.as_bytes()))
    }
}

pub fn generate_client_key(client: GatewayClient) -> anyhow::Result<String> {
    let mut bytes = [0_u8; 32];
    rand::SecureRandom::fill(&rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| anyhow::anyhow!("generate gateway client key"))?;
    Ok(format!(
        "cmc1_{}_{}",
        client.as_str(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticates_only_the_installed_client_keys() {
        let keys = GatewayClientKeys {
            codex: Some("codex-key".to_owned()),
            claude: Some("claude-key".to_owned()),
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(CODEX_CLIENT_KEY_HEADER, "codex-key".parse().unwrap());
        assert_eq!(keys.authenticate(&headers), Some(GatewayClient::Codex));
        headers.clear();
        headers.insert(header::AUTHORIZATION, "Bearer claude-key".parse().unwrap());
        assert_eq!(keys.authenticate(&headers), Some(GatewayClient::Claude));
        headers.insert(header::AUTHORIZATION, "Bearer copied-key".parse().unwrap());
        assert_eq!(keys.authenticate(&headers), None);

        let keys = GatewayClientKeys {
            pi: Some("pi-key".to_owned()),
            ..Default::default()
        };
        headers.insert(header::AUTHORIZATION, "Bearer pi-key".parse().unwrap());
        assert_eq!(keys.authenticate(&headers), Some(GatewayClient::Pi));
    }

    #[test]
    fn generated_keys_are_independent() {
        let codex = generate_client_key(GatewayClient::Codex).unwrap();
        let claude = generate_client_key(GatewayClient::Claude).unwrap();
        assert_ne!(codex, claude);
        assert!(codex.starts_with("cmc1_codex_"));
    }
}
