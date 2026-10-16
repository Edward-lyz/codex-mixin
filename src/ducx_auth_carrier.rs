use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "CODEX_MIXIN_DUCX_AUTH_V1=";
pub const VERSION: u8 = 1;

#[derive(Deserialize, Serialize)]
pub struct DucxAuthCarrier {
    pub version: u8,
    pub model_token: String,
    pub custom_header: String,
}
