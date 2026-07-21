use super::types::{FusionDetail, FusionPanelDetail, FusionPanelStatus, RenderedFusionDetail};
use super::*;

const PANEL_RESULTS_TITLE: &str = "Fusion · Panel Results";
const JUDGE_RESULT_TITLE: &str = "Fusion · Judge Result";
const FINAL_ANSWER_TITLE: &str = "Fusion · Final Answer";

pub(super) fn progress_event(model: &str, delta: &str) -> Bytes {
    encode_event(
        "response.reasoning_summary_text.delta",
        &json!({
            "type":"response.reasoning_summary_text.delta",
            "item_id":"fusion_progress",
            "output_index":0,
            "summary_index":0,
            "delta":delta,
            "model":model
        }),
    )
    .expect("fusion progress event is serializable")
}

pub(super) fn render_fusion_details(details: &[FusionDetail]) -> Vec<RenderedFusionDetail> {
    details
        .iter()
        .enumerate()
        .map(|(output_index, detail)| {
            let item_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
            let text = format!("## {}\n\n{}", detail.title, detail.text);
            let completed_item = json!({
                "id":item_id,
                "type":"message",
                "status":"completed",
                "role":"assistant",
                "content":[{"type":"output_text","text":text,"annotations":[]}]
            });
            let events = vec![
                encode_event(
                    "response.output_item.added",
                    &json!({
                        "type":"response.output_item.added",
                        "output_index":output_index,
                        "item":{
                            "id":item_id,
                            "type":"message",
                            "status":"in_progress",
                            "role":"assistant",
                            "content":[]
                        }
                    }),
                )
                .expect("fusion detail item is serializable"),
                encode_event(
                    "response.content_part.added",
                    &json!({
                        "type":"response.content_part.added",
                        "item_id":item_id,
                        "output_index":output_index,
                        "content_index":0,
                        "part":{"type":"output_text","text":"","annotations":[]}
                    }),
                )
                .expect("fusion detail part is serializable"),
                encode_event(
                    "response.output_text.delta",
                    &json!({
                        "type":"response.output_text.delta",
                        "item_id":item_id,
                        "output_index":output_index,
                        "content_index":0,
                        "delta":text
                    }),
                )
                .expect("fusion detail delta is serializable"),
                encode_event(
                    "response.output_text.done",
                    &json!({
                        "type":"response.output_text.done",
                        "item_id":item_id,
                        "output_index":output_index,
                        "content_index":0,
                        "text":text
                    }),
                )
                .expect("fusion detail text is serializable"),
                encode_event(
                    "response.content_part.done",
                    &json!({
                        "type":"response.content_part.done",
                        "item_id":item_id,
                        "output_index":output_index,
                        "content_index":0,
                        "part":{"type":"output_text","text":text,"annotations":[]}
                    }),
                )
                .expect("fusion detail part is serializable"),
                encode_event(
                    "response.output_item.done",
                    &json!({
                        "type":"response.output_item.done",
                        "output_index":output_index,
                        "item":completed_item
                    }),
                )
                .expect("fusion detail item is serializable"),
            ];
            RenderedFusionDetail {
                item: completed_item,
                events,
            }
        })
        .collect()
}

pub(super) fn panel_results_detail(panels: &[FusionPanelDetail]) -> FusionDetail {
    let rows = panels
        .iter()
        .map(|panel| {
            let (status, summary) = match panel.status {
                FusionPanelStatus::Completed => ("Completed", "Show output"),
                FusionPanelStatus::Failed => ("Failed", "Show error"),
            };
            format!(
                "| {} | <code>{}</code> | {status} | <details><summary>{summary}</summary><br>{}</details> |",
                panel.index + 1,
                escape_table_cell(&panel.model),
                escape_table_cell(&panel.text)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    FusionDetail {
        title: PANEL_RESULTS_TITLE.to_owned(),
        text: format!("| # | Panel Model | Status | Result |\n| --: | --- | --- | --- |\n{rows}"),
    }
}

pub(super) fn judge_result_detail(model: &str, status: &str, text: String) -> FusionDetail {
    FusionDetail {
        title: JUDGE_RESULT_TITLE.to_owned(),
        text: format!(
            "**Model:** <code>{}</code>\n\n**Status:** {status}\n\n{text}",
            escape_html(model)
        ),
    }
}

pub(super) fn final_answer_detail(model: &str) -> FusionDetail {
    FusionDetail {
        title: FINAL_ANSWER_TITLE.to_owned(),
        text: format!("**Model:** <code>{}</code>", escape_html(model)),
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_table_cell(value: &str) -> String {
    escape_html(value)
        .replace('|', "&#124;")
        .replace("\r\n", "<br>")
        .replace(['\r', '\n'], "<br>")
}

pub(super) fn patch_final_event(
    payload: &mut Value,
    output_offset: u64,
    detail_items: &[Value],
    downstream_model: &str,
) {
    if let Some(output_index) = payload.get_mut("output_index")
        && let Some(index) = output_index.as_u64()
    {
        *output_index = json!(index + output_offset);
    }
    if let Some(response) = payload.get_mut("response") {
        response["model"] = Value::String(downstream_model.to_owned());
    }
    if !matches!(
        payload.get("type").and_then(Value::as_str),
        Some("response.completed" | "response.failed" | "response.incomplete")
    ) {
        return;
    }
    let Some(output) = payload
        .get_mut("response")
        .and_then(|response| response.get_mut("output"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let final_items = std::mem::take(output);
    output.reserve(detail_items.len() + final_items.len());
    output.extend_from_slice(detail_items);
    output.extend(final_items);
}

pub(super) fn failed_event(model: &str, message: &str) -> Bytes {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    let error = json!({"message":message,"type":"server_error"});
    encode_event(
        "response.failed",
        &json!({
            "type":"response.failed",
            "response":{
                "id":response_id,
                "object":"response",
                "status":"failed",
                "model":model,
                "error":error,
                "output":[]
            },
            "error":error
        }),
    )
    .expect("fusion failure event is serializable")
}
