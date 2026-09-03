use serde_json::{Value, json};

use crate::error::GatewayError;
use crate::protocol::convert::{
    ToolNameMap, agent_message_text, collect_active_tools, custom_tool_description,
    sanitize_tool_name,
};

mod content;
mod request;
mod tools;

pub(crate) use request::responses_to_openai_chat_streaming_with_model;
pub use request::{ConvertedChatRequest, responses_to_openai_chat};

#[cfg(test)]
mod tests;
