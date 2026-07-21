use serde_json::json;

use crate::config::ProviderPreset;

use super::render::{panel_results_detail, render_fusion_details};
use super::routing::{FusionModelProvider, resolve_fusion_model};
use super::types::{FusionPanelDetail, FusionPanelStatus};
use super::*;

fn profile() -> FusionProfile {
    FusionProfile {
        id: "default".to_owned(),
        panel_models: vec!["a".to_owned(), "b".to_owned()],
        judge_model: "judge".to_owned(),
        final_model: "final".to_owned(),
        min_successful: 2,
        max_completion_tokens: 2048,
        timeout_ms: 300_000,
        fuse_every_user_turn: true,
        show_intermediate_results: true,
        panel_tools: PanelToolsConfig::default(),
    }
}

#[test]
fn validates_panel_bounds_recursion_and_minimum() {
    let mut value = profile();
    assert!(value.validate().is_ok());
    value.panel_models.clear();
    assert!(value.validate().is_err());
    value = profile();
    value.panel_models = (0..9).map(|index| format!("panel-{index}")).collect();
    assert!(value.validate().is_err());
    value = profile();
    value.panel_models[0] = "mixin/fusion/other".to_owned();
    assert!(value.validate().is_err());
    value = profile();
    value.min_successful = 3;
    assert!(value.validate().is_err());
}

#[test]
fn routes_provider_suffixed_gpt_aliases_to_upstream() {
    assert_eq!(model_route("gpt-5.6-sol"), ModelRoute::Official);
    for provider in ProviderPreset::ALL {
        let alias = format!("gpt-5.6-sol-{}", provider.as_str());
        assert_eq!(model_route(&alias), ModelRoute::Direct);
        assert_eq!(canonical_upstream_model_alias(&alias), "gpt-5.6-sol");
    }
    assert_eq!(
        resolve_fusion_model("official:gpt-5.6-sol", "baidu-oneapi"),
        (FusionModelProvider::Official, "gpt-5.6-sol".to_owned())
    );
    assert_eq!(
        resolve_fusion_model("baidu-oneapi:gpt-5.6-sol", "baidu-oneapi"),
        (FusionModelProvider::Upstream, "gpt-5.6-sol".to_owned())
    );
}

#[test]
fn accepts_structured_and_plain_text_panel_reports() {
    let valid = r#"{"findings":["main.rs mixes unrelated responsibilities"],"risks":["maintenance cost"],"recommendations":["extract install logic"],"evidence":["main.rs has over 3000 lines"]}"#;
    assert!(normalize_panel_analysis(valid).is_ok());
    assert!(
        normalize_panel_analysis("The file is large and should be split by responsibility.")
            .is_ok()
    );
    assert!(
        normalize_panel_analysis(
            r#"<｜｜DSML｜｜tool_calls><｜｜DSML｜｜invoke name="read_file">"#
        )
        .is_err()
    );
    assert!(
        normalize_panel_analysis(
            r#"{"findings":[],"risks":[],"recommendations":[],"evidence":[]}"#
        )
        .is_err()
    );
}

#[test]
fn panel_requests_allow_plain_text_and_default_to_five_minutes() {
    let request = panel_request(&profile(), "panel-a", "analyze", false);
    assert!(request.get("text").is_none());
    let parsed: FusionProfile = serde_json::from_value(json!({
        "id":"default",
        "panel_models":["panel-a"],
        "judge_model":"judge",
        "final_model":"final"
    }))
    .unwrap();
    assert_eq!(parsed.timeout_ms, 300_000);
    assert!(parsed.show_intermediate_results);
}

#[test]
fn renders_panel_outputs_in_one_collapsible_table() {
    let detail = panel_results_detail(&[
        FusionPanelDetail {
            index: 0,
            model: "panel<a>".to_owned(),
            status: FusionPanelStatus::Completed,
            text: "line one | detail\nline two".to_owned(),
        },
        FusionPanelDetail {
            index: 1,
            model: "panel-b".to_owned(),
            status: FusionPanelStatus::Failed,
            text: "upstream failed".to_owned(),
        },
    ]);
    let rendered = render_fusion_details(&[detail]);
    let text = rendered[0].item["content"][0]["text"].as_str().unwrap();

    assert!(text.starts_with("## Fusion · Panel Results"));
    assert!(text.contains("| # | Panel Model | Status | Result |"));
    assert!(text.contains("<code>panel&lt;a&gt;</code>"));
    assert!(text.contains("<details><summary>Show output</summary>"));
    assert!(text.contains("line one &#124; detail<br>line two"));
    assert!(text.contains("<details><summary>Show error</summary>"));
}

#[test]
fn detects_user_turns_and_tool_continuations() {
    assert!(should_fuse_turn(&json!({"input":[
        {"type":"message","role":"assistant"},
        {"type":"message","role":"user"}
    ]})));
    assert!(should_fuse_turn(&json!({
        "previous_response_id":"resp_1",
        "input":[
            {"type":"message","role":"assistant"},
            {"type":"message","role":"user","content":"<collaboration_mode>Default</collaboration_mode> write code"}
        ]
    })));
    assert!(!should_fuse_turn(&json!({"input":[
        {"type":"message","role":"user"},
        {"type":"function_call_output"}
    ]})));
    assert!(should_fuse_turn(&json!({"input":[
        {"type":"function_call_output"},
        {"type":"message","role":"user"}
    ]})));
}
