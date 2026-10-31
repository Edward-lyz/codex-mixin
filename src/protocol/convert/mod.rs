use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::anthropic::{ContentBlock, Message, MessageRequest, Tool};
use crate::config::ThinkingMode;
use crate::error::GatewayError;

mod content;
mod request;
mod thinking;
mod tool_map;
mod tools;

pub(crate) use content::{
    agent_message_text, encode_anthropic_redacted_thinking, encode_anthropic_thinking,
    encode_anthropic_unsigned_thinking, encode_baidu_unsigned_thinking, gateway_thinking_text,
    is_gateway_thinking,
};
pub use request::{ConvertedRequest, responses_to_anthropic};
pub(crate) use request::{
    native_messages_body, responses_to_anthropic_with_model_reasoning_and_thinking_kind,
};
pub use tool_map::ToolNameMap;
pub use tools::sanitize_tool_name;
pub(crate) use tools::{WebSearchTool, collect_active_tools, custom_tool_description};
