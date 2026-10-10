use super::state::{AssistantMessagePhase, MapperState, TextBlock};
use super::*;

impl MapperState {
    pub(super) fn chat_reasoning_delta(&mut self, delta: &str) -> Vec<Bytes> {
        let mut events = Vec::new();
        if self.chat_reasoning.is_none() {
            events.extend(self.finish_text(AssistantMessagePhase::Commentary));
            let output_index = self.output.len();
            let item_id = format!("rs_{}", Uuid::new_v4().simple());
            let item = json!({"id":item_id,"type":"reasoning","status":"in_progress","summary":[],"content":[]});
            self.output.push(item.clone());
            events.push(chat_event(
                "response.output_item.added",
                json!({
                    "type":"response.output_item.added","output_index":output_index,"item":item
                }),
            ));
            events.push(chat_event("response.content_part.added", json!({
                "type":"response.content_part.added","item_id":item_id,"output_index":output_index,
                "content_index":0,"part":{"type":"reasoning_text","text":""}
            })));
            self.chat_reasoning = Some(TextBlock {
                output_index,
                item_id,
                text: String::new(),
            });
        }
        if let Some(block) = &mut self.chat_reasoning {
            block.text.push_str(delta);
            // Chat reasoning_content is raw text, not a generated summary.
            events.push(chat_event(
                "response.reasoning_text.delta",
                json!({
                    "type":"response.reasoning_text.delta","item_id":block.item_id,
                    "output_index":block.output_index,"content_index":0,"delta":delta
                }),
            ));
        }
        events
    }

    pub(super) fn finish_chat_reasoning(&mut self) -> Vec<Bytes> {
        let Some(block) = self.chat_reasoning.take() else {
            return Vec::new();
        };
        let part = json!({"type":"reasoning_text","text":block.text});
        let item = json!({"id":block.item_id,"type":"reasoning","status":"completed",
            "summary":[],"content":[part]});
        self.output[block.output_index] = item.clone();
        vec![
            chat_event(
                "response.reasoning_text.done",
                json!({
                    "type":"response.reasoning_text.done","item_id":block.item_id,
                    "output_index":block.output_index,"content_index":0,"text":block.text
                }),
            ),
            chat_event(
                "response.content_part.done",
                json!({
                    "type":"response.content_part.done","item_id":block.item_id,
                    "output_index":block.output_index,"content_index":0,"part":part
                }),
            ),
            chat_event(
                "response.output_item.done",
                json!({
                    "type":"response.output_item.done","output_index":block.output_index,"item":item
                }),
            ),
        ]
    }
}
