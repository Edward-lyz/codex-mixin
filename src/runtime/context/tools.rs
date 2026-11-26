use serde_json::{Value, json};

use super::types::{TOOL_COMPRESS, TOOL_DECOMPRESS, TOOL_SEARCH, TOOL_STATUS};

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "name": TOOL_COMPRESS,
            "description": "Replace a contiguous range of older context segments with a dense factual summary. Provide either ranges of segment references or block ids.",
            "parameters": {
                "type": "object",
                "properties": {
                    "ranges": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "start": {"type": "string"},
                                "end": {"type": "string"}
                            },
                            "required": ["start", "end"],
                            "additionalProperties": false
                        }
                    },
                    "block_ids": {"type": "array", "items": {"type": "string"}},
                    "topic": {"type": "string"},
                    "summary": {"type": "string"}
                },
                "required": ["topic", "summary"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": TOOL_DECOMPRESS,
            "description": "Restore a summary block into the current view, either as the block itself or as its original segments.",
            "parameters": {
                "type": "object",
                "properties": {
                    "block_id": {"type": "string"},
                    "apply": {"type": "boolean"},
                    "offset": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["block_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": TOOL_SEARCH,
            "description": "Search archived summaries and original messages of this session.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "type": "function",
            "name": TOOL_STATUS,
            "description": "Report context window pressure, protected segments, tier counts and compressible ranges.",
            "parameters": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
    ]
}
