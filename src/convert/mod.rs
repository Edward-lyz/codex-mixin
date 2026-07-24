use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::anthropic::{ContentBlock, Message, MessageRequest, Tool};
use crate::config::{GatewayConfig, ThinkingMode};
use crate::error::GatewayError;

mod content;
mod request;
mod thinking;
mod tool_map;
mod tools;

pub(crate) use content::agent_message_text;
pub(crate) use request::responses_to_anthropic_with_web_search;
pub use request::{ConvertedRequest, responses_to_anthropic};
pub use tool_map::ToolNameMap;
pub use tools::sanitize_tool_name;
pub(crate) use tools::{collect_active_tools, custom_tool_description};

#[cfg(test)]
mod tests;
