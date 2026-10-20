use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u64,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<ContentBlock>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "image")]
    Image { source: Value },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: Value },
}

pub type Tool = Value;

#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    #[serde(deserialize_with = "deserialize_catalog_models")]
    pub data: Vec<ModelInfo>,
}

fn deserialize_catalog_models<'de, D>(deserializer: D) -> Result<Vec<ModelInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Catalogs may repeat keys or emit both canonical names and their aliases.
    // Value keeps the last duplicate; canonical non-null metadata wins aliases.
    let models = Vec::<Value>::deserialize(deserializer)?;
    models
        .into_iter()
        .map(|mut model| {
            if let Some(fields) = model.as_object_mut() {
                for (canonical, aliases) in [
                    ("display_name", &["name"][..]),
                    ("context_window", &["context_length"][..]),
                    (
                        "max_output_tokens",
                        &["max_tokens", "max_completion_tokens"][..],
                    ),
                ] {
                    for alias in aliases {
                        if let Some(value) = fields.remove(*alias)
                            && fields.get(canonical).is_none_or(Value::is_null)
                        {
                            fields.insert(canonical.to_owned(), value);
                        }
                    }
                }
            }
            serde_json::from_value(model).map_err(serde::de::Error::custom)
        })
        .collect()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, alias = "name", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_type: Option<String>,
    #[serde(
        default,
        alias = "context_length",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<u64>,
    #[serde(
        default,
        alias = "max_tokens",
        alias = "max_completion_tokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<crate::provider::ProviderProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_web_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_function_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities_probed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<OpenRouterModelArchitecture>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_parameters: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OpenRouterModelArchitecture {
    #[serde(default)]
    pub input_modalities: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct BaiduAvailableModelsResponse {
    pub data: Vec<BaiduAvailableModel>,
    pub success: bool,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct BaiduAvailableModel {
    pub model: String,
    pub capability: Option<BaiduModelCapability>,
    #[serde(default)]
    pub capability_set: Vec<String>,
    pub price_type: String,
}

#[derive(Debug, Deserialize)]
pub struct BaiduModelCapability {
    pub supports_image: bool,
    pub supports_thinking: bool,
    pub context_window: u64,
    pub ratio: String,
    pub model_description: String,
}

#[cfg(test)]
mod tests {
    use super::ModelsResponse;

    #[test]
    fn models_accept_duplicate_metadata() {
        let response: ModelsResponse = serde_json::from_str(
            r#"{"data":[{"id":"beam","context_window":32000,"context_window":128000}]}"#,
        )
        .unwrap();
        assert_eq!(response.data[0].context_window, Some(128000));
    }

    #[test]
    fn models_accept_coexisting_aliases() {
        for fields in [
            r#""context_window":128000,"context_length":64000,"display_name":"Beam","name":"alias","max_output_tokens":8192,"max_tokens":4096,"max_completion_tokens":2048"#,
            r#""context_length":64000,"context_window":128000,"name":"alias","display_name":"Beam","max_completion_tokens":2048,"max_tokens":4096,"max_output_tokens":8192"#,
        ] {
            let body = format!(r#"{{"data":[{{"id":"beam",{fields}}}]}}"#);
            // Provider discovery parses text; provider setup parses a Value first.
            let text: ModelsResponse = serde_json::from_str(&body).unwrap();
            let value: ModelsResponse =
                serde_json::from_value(serde_json::from_str::<serde_json::Value>(&body).unwrap())
                    .unwrap();
            for response in [text, value] {
                let model = &response.data[0];
                assert_eq!(model.context_window, Some(128000));
                assert_eq!(model.max_output_tokens, Some(8192));
                assert_eq!(model.display_name.as_deref(), Some("Beam"));
            }
        }
    }

    #[test]
    fn models_accept_aliases_without_canonical_values() {
        let response: ModelsResponse = serde_json::from_str(
            r#"{"data":[{"id":"beam","display_name":null,"name":"Beam","context_window":null,"context_length":64000,"max_output_tokens":null,"max_tokens":4096,"max_completion_tokens":2048},{"id":"legacy","max_completion_tokens":1024}]}"#,
        )
        .unwrap();
        assert_eq!(response.data[0].display_name.as_deref(), Some("Beam"));
        assert_eq!(response.data[0].context_window, Some(64000));
        assert_eq!(response.data[0].max_output_tokens, Some(4096));
        assert_eq!(response.data[1].max_output_tokens, Some(1024));
    }

    #[test]
    fn models_reject_invalid_records() {
        for body in [
            r#"{"data":[{}]}"#,
            r#"{"data":[{"id":42}]}"#,
            r#"{"data":[{"id":"beam","context_window":"invalid","context_length":64000}]}"#,
            r#"{"data":[{"id":"beam","context_length":-1}]}"#,
            r#"{"data":[null]}"#,
            r#"{"data":null}"#,
            r#"{"data":[{"id":"beam",}]}"#,
        ] {
            assert!(
                serde_json::from_str::<ModelsResponse>(body).is_err(),
                "{body}"
            );
        }
    }
}
