use serde_json::{Value, json};

use crate::convert::{
    ToolNameMap, agent_message_text, collect_active_tools, custom_tool_description,
    sanitize_tool_name,
};
use crate::error::GatewayError;

mod content;
mod request;
mod tools;

pub use request::{ConvertedChatRequest, responses_to_openai_chat};

#[cfg(test)]
mod tests;
