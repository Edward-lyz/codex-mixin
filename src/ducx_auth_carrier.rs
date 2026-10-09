use serde::{Deserialize, Serialize};

pub(crate) const PREFIX: &str = "CODEX_MIXIN_DUCX_AUTH_V1=";
pub(crate) const VERSION: u8 = 1;

#[derive(Deserialize, Serialize)]
pub(crate) struct DucxAuthCarrier {
    pub(crate) version: u8,
    pub(crate) model_token: String,
    pub(crate) custom_header: String,
}
