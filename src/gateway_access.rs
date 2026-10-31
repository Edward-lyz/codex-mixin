use axum::http::{HeaderMap, header};
use base64::Engine;
use ring::rand;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

pub const CODEX_CLIENT_KEY_HEADER: &str = "x-codex-mixin-client-key";
// Set only by the inbound adapter after stripping client-supplied copies.
pub(crate) const LOOPBACK_PEER_HEADER: &str = "x-codex-mixin-internal-loopback-peer";

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayClient {
    Codex,
    Claude,
    ClaudeDesktop,
    Dsh,
    OpenCode,
    Pi,
    Framework(&'static str),
}

impl GatewayClient {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Dsh => "dsh",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Framework(id) => id,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Dsh => "DSH",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Framework(id) => id,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayClientKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub frameworks: std::collections::BTreeMap<String, Option<String>>,
}

impl GatewayClientKeys {
    pub fn get(&self, client: GatewayClient) -> Option<&str> {
        match client {
            GatewayClient::Codex => self.codex.as_deref(),
            GatewayClient::Claude => self.claude.as_deref(),
            GatewayClient::ClaudeDesktop => self.claude_desktop.as_deref(),
            GatewayClient::Dsh => self.dsh.as_deref(),
            GatewayClient::OpenCode => self.opencode.as_deref(),
            GatewayClient::Pi => self.pi.as_deref(),
            GatewayClient::Framework(id) => self.frameworks.get(id).and_then(Option::as_deref),
        }
    }

    pub fn get_mut(&mut self, client: GatewayClient) -> &mut Option<String> {
        match client {
            GatewayClient::Codex => &mut self.codex,
            GatewayClient::Claude => &mut self.claude,
            GatewayClient::ClaudeDesktop => &mut self.claude_desktop,
            GatewayClient::Dsh => &mut self.dsh,
            GatewayClient::OpenCode => &mut self.opencode,
            GatewayClient::Pi => &mut self.pi,
            GatewayClient::Framework(id) => self.frameworks.entry(id.to_owned()).or_default(),
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
            .and_then(|value| value.strip_prefix("Bearer "))
            .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()));
        [
            GatewayClient::Claude,
            GatewayClient::ClaudeDesktop,
            GatewayClient::Dsh,
            GatewayClient::OpenCode,
            GatewayClient::Pi,
        ]
        .into_iter()
        .find(|client| self.matches(*client, bearer))
        .or_else(|| {
            crate::application::agent_versions::AGENT_VERSION_IDS
                .into_iter()
                .map(GatewayClient::Framework)
                .find(|client| self.matches(*client, bearer))
        })
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
