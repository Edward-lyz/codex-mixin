use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "CODEX_MIXIN_DUCX_AUTH_V1=";
pub const VERSION: u8 = 1;

/// Environment switches that stop DUCX from updating itself. Newer DUCX
/// builds read `DISABLE_BAIDU_CODEX_UPDATE`; older ones read the other two.
/// A self-update mid-session leaves a half-downloaded version whose login
/// state the active install cannot read, so every DUCX process gets all three.
pub const UPDATE_GUARDS: [&str; 3] = [
    "DISABLE_DUCX_CLI_UPDATE",
    "DISABLE_BAIDU_CODEX_UPDATE",
    "DISABLE_BAIDU_CLAUDE_UPDATE",
];

#[derive(Deserialize, Serialize)]
pub struct DucxAuthCarrier {
    pub version: u8,
    pub model_token: String,
    pub custom_header: String,
}
