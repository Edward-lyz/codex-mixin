pub(super) const FALLBACK_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. Work in the user's workspace, use tools carefully, and keep responses concise.";
pub(super) const CUSTOM_MODEL_MARKER: &str = "codex_mixin_managed";
pub(super) const UPSTREAM_MODEL_MARKER: &str = "codex_mixin_upstream_model";
pub(super) const SUPPORTS_THINKING_MARKER: &str = "codex_mixin_supports_thinking";

mod generation;
mod managed;
mod template;

pub use generation::{
    codex_catalog_from_models, codex_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_aggregated_models_with_metadata,
    codex_oauth_proxy_catalog_from_models, codex_oauth_proxy_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_models_with_metadata_for_provider,
};
pub use managed::{migrate_managed_model_metadata, refresh_managed_oauth_catalog};
pub use template::{apply_web_search_capabilities, load_template_catalog};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::{Value, json};

    use crate::anthropic::ModelInfo;
    use crate::provider::MetadataResolver;

    use super::{
        CUSTOM_MODEL_MARKER, FALLBACK_BASE_INSTRUCTIONS, SUPPORTS_THINKING_MARKER,
        UPSTREAM_MODEL_MARKER, apply_web_search_capabilities, codex_catalog_from_models,
        codex_catalog_from_models_with_metadata,
        codex_oauth_proxy_catalog_from_aggregated_models_with_metadata,
        codex_oauth_proxy_catalog_from_models,
        codex_oauth_proxy_catalog_from_models_with_metadata_for_provider,
        refresh_managed_oauth_catalog,
    };

    fn reasoning_efforts(model: &Value) -> Vec<&str> {
        model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level["effort"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn builds_codex_catalog_from_live_model_shape() {
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        assert_eq!(catalog["models"][0]["slug"], "DeepSeek-V4-Flash");
        assert_eq!(
            catalog["models"][0]["additional_speed_tiers"],
            json!(["fast"])
        );
        assert_eq!(catalog["models"][0]["service_tiers"][0]["id"], "priority");
        assert_eq!(
            catalog["models"][0]["base_instructions"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(
            catalog["models"][0]["model_messages"]["instructions_template"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(catalog["models"][0]["context_window"], 1_000_000);
        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
        assert!(catalog["models"][0].get("web_search_tool_type").is_none());
        assert_eq!(
            reasoning_efforts(&catalog["models"][0]),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(catalog["models"][0]["multi_agent_version"], "v2");
    }

    #[test]
    fn applies_model_metadata_to_context_window_and_modalities() {
        let metadata = MetadataResolver::from_json(&json!({
            "fireworks_ai/minimax-m3": {
                "mode": "chat",
                "max_input_tokens": 512000,
                "max_output_tokens": 512000,
                "supports_vision": true
            }
        }))
        .unwrap();
        let models = vec![ModelInfo {
            id: "MiniMax-M3".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models_with_metadata(&models, 1_000_000, None, &metadata);
        assert_eq!(catalog["models"][0]["context_window"], 512_000);
        assert_eq!(catalog["models"][0]["max_context_window"], 512_000);
        assert_eq!(
            catalog["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
    }

    #[test]
    fn provider_metadata_overrides_catalog_description_and_capabilities() {
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            description: Some("Fast coding model".to_owned()),
            ratio: Some("0.2x".to_owned()),
            price_type: Some("Value".to_owned()),
            context_window: Some(1_024_000),
            supports_image: Some(false),
            supports_thinking: Some(true),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, None);

        assert_eq!(
            catalog["models"][0]["description"],
            "Fast coding model | 0.2x | Value"
        );
        assert_eq!(catalog["models"][0]["context_window"], 1_024_000);
        assert_eq!(catalog["models"][0]["input_modalities"], json!(["text"]));
        assert_eq!(catalog["models"][0][CUSTOM_MODEL_MARKER], true);
        assert_eq!(
            reasoning_efforts(&catalog["models"][0]),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(catalog["models"][0]["multi_agent_version"], "v2");
    }

    #[test]
    fn applies_known_gpt_and_claude_reasoning_profiles() {
        let models = [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "Claude Opus 4.6",
        ]
        .into_iter()
        .map(|id| ModelInfo {
            id: id.to_owned(),
            supports_thinking: Some(true),
            ..ModelInfo::default()
        })
        .collect::<Vec<_>>();

        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        let models = catalog["models"].as_array().unwrap();
        let sol = &models[0];
        assert_eq!(sol["default_reasoning_level"], "low");
        assert_eq!(
            reasoning_efforts(sol),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(sol["multi_agent_version"], "v2");

        let terra = &models[1];
        assert_eq!(terra["default_reasoning_level"], "medium");
        assert_eq!(
            reasoning_efforts(terra),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(terra["multi_agent_version"], "v2");

        let luna = &models[2];
        assert_eq!(
            reasoning_efforts(luna),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(luna["multi_agent_version"], "v2");

        let gpt_5_5 = &models[3];
        assert_eq!(
            reasoning_efforts(gpt_5_5),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(gpt_5_5["multi_agent_version"], "v2");

        let opus = &models[4];
        assert_eq!(
            reasoning_efforts(opus),
            ["none", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(opus["multi_agent_version"], "v2");
    }

    #[test]
    fn applies_advertised_reasoning_to_other_model_families() {
        let models = [
            "DeepSeek-V4-Flash",
            "GLM-5.2",
            "Kimi-K2.7-Code",
            "MiniMax-M3",
            "grok-4.5",
        ]
        .into_iter()
        .map(|id| ModelInfo {
            id: id.to_owned(),
            supports_thinking: Some(true),
            ..ModelInfo::default()
        })
        .collect::<Vec<_>>();

        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        for model in catalog["models"].as_array().unwrap() {
            assert_eq!(
                reasoning_efforts(model),
                ["none", "low", "medium", "high", "xhigh", "max", "ultra"],
                "{}",
                model["slug"]
            );
            assert_eq!(model["multi_agent_version"], "v2");
            assert_eq!(model[SUPPORTS_THINKING_MARKER], true);
        }
    }

    #[test]
    fn keeps_off_and_ultra_when_provider_reports_no_thinking_support() {
        let template = json!({"models":[{
            "slug":"gpt-5.4-mini",
            "default_reasoning_level":"medium",
            "supported_reasoning_levels":[{"effort":"medium","description":"Inherited"}],
            "multi_agent_version":"v2"
        }]});
        let models = vec![ModelInfo {
            id: "plain-model".to_owned(),
            supports_thinking: Some(false),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, Some(&template));
        let model = &catalog["models"][0];
        assert_eq!(model["default_reasoning_level"], "none");
        assert_eq!(reasoning_efforts(model), ["none", "ultra"]);
        assert_eq!(model["multi_agent_version"], "v2");
    }

    #[test]
    fn keeps_hosted_web_search_capability_separate_from_tool_search() {
        let models = vec![ModelInfo {
            id: "Claude Sonnet 5".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            supports_web_search: Some(true),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
        assert_eq!(catalog["models"][0]["web_search_tool_type"], "text");
    }

    #[test]
    fn removes_inherited_search_type_from_unsupported_models() {
        let template = json!({
            "models": [{
                "slug": "gpt-template",
                "display_name": "Template",
                "base_instructions": "test",
                "web_search_tool_type": "text_and_image"
            }]
        });
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, Some(&template));

        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
        assert!(catalog["models"][0].get("web_search_tool_type").is_none());
    }

    #[test]
    fn removes_inherited_official_lifecycle_metadata_from_custom_models() {
        let template = json!({
            "models": [{
                "slug": "gpt-5.4-mini",
                "display_name": "GPT-5.4 Mini",
                "upgrade": {
                    "model": "gpt-5.6-luna",
                    "migration_markdown": "Use GPT-5.6 Luna."
                },
                "availability_nux": {"message": "Official model notice"},
                "retirement_at": "2026-08-31T00:00:00Z",
                "migration_markdown": "Legacy top-level notice"
            }]
        });
        let models = vec![ModelInfo {
            id: "custom-model".to_owned(),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, Some(&template));
        let model = &catalog["models"][0];

        for field in [
            "upgrade",
            "availability_nux",
            "retirement_at",
            "migration_markdown",
        ] {
            assert!(model.get(field).is_none(), "inherited {field}");
        }
    }

    #[test]
    fn applies_probed_web_search_capabilities_only_to_custom_models() {
        let mut catalog = json!({
            "models": [
                {"slug":"gpt-5.5","web_search_tool_type":"text"},
                {"slug":"gpt-5.6-sol-custom","codex_mixin_managed":true,"web_search_tool_type":"text_and_image","use_responses_lite":true},
                {"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true,"web_search_tool_type":"text"},
                {"slug":"user-model-custom","web_search_tool_type":"text_and_image","use_responses_lite":true}
            ]
        });
        let supported = HashSet::from(["gpt-5.6-sol".to_owned()]);

        assert!(apply_web_search_capabilities(&mut catalog, &supported).unwrap());
        assert_eq!(catalog["models"][0]["web_search_tool_type"], "text");
        assert_eq!(catalog["models"][1]["web_search_tool_type"], "text");
        assert_eq!(catalog["models"][1]["use_responses_lite"], false);
        assert!(catalog["models"][2].get("web_search_tool_type").is_none());
        assert_eq!(
            catalog["models"][3]["web_search_tool_type"],
            "text_and_image"
        );
        assert_eq!(catalog["models"][3]["use_responses_lite"], true);
    }

    #[test]
    fn oauth_proxy_catalog_keeps_official_gpt_and_aliases_custom_gpt() {
        let template = json!({
            "models": [
                {"slug":"gpt-5.5","display_name":"GPT-5.5","context_window":272000}
            ]
        });
        let models = vec![ModelInfo {
            id: "gpt-5.5".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_oauth_proxy_catalog_from_models(&models, 1_000_000, Some(&template));
        assert_eq!(catalog["models"][0]["slug"], "gpt-5.5");
        assert_eq!(catalog["models"][1]["slug"], "gpt-5.5-custom");
        assert_eq!(catalog["models"][1]["display_name"], "gpt-5.5 (Custom)");
        assert_eq!(
            catalog["models"][1]["base_instructions"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(
            catalog["models"][1]["model_messages"]["instructions_template"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(catalog["models"][1]["multi_agent_version"], "v2");
    }

    #[test]
    fn oauth_proxy_catalog_uses_provider_suffix_for_gpt_collisions() {
        let template = json!({"models":[{
            "slug":"gpt-5.5",
            "display_name":"GPT-5.5",
            "context_window":272000,
            "max_context_window":272000
        }]});
        let models = vec![ModelInfo {
            id: "gpt-5.5".to_owned(),
            context_window: Some(372_000),
            ..ModelInfo::default()
        }];
        let metadata = MetadataResolver::empty();
        let catalog = codex_oauth_proxy_catalog_from_models_with_metadata_for_provider(
            &models,
            1_000_000,
            Some(&template),
            &metadata,
            "baidu-oneapi",
        );
        assert_eq!(catalog["models"][0]["slug"], "gpt-5.5");
        assert_eq!(catalog["models"][1]["slug"], "gpt-5.5-baidu-oneapi");
        assert_eq!(catalog["models"][1][UPSTREAM_MODEL_MARKER], "gpt-5.5");
        // Provider-advertised windows win over official GPT template inheritance.
        assert_eq!(catalog["models"][1]["context_window"], 372_000);
        assert_eq!(catalog["models"][1]["max_context_window"], 372_000);
    }

    #[test]
    fn aggregated_oauth_catalog_does_not_suffix_provider_qualified_gpt_twice() {
        let template = json!({"models":[{
            "slug":"gpt-5.6-sol",
            "display_name":"5.6 Sol",
            "context_window":272000
        }]});
        let models = vec![ModelInfo {
            id: "gpt-5.6-sol-custom-2".to_owned(),
            display_name: Some("5.6 Sol · AIHub".to_owned()),
            ..ModelInfo::default()
        }];
        let metadata = MetadataResolver::empty();

        let catalog = codex_oauth_proxy_catalog_from_aggregated_models_with_metadata(
            &models,
            1_000_000,
            Some(&template),
            &metadata,
        );

        assert_eq!(catalog["models"][1]["slug"], "gpt-5.6-sol-custom-2");
        assert_eq!(catalog["models"][1]["display_name"], "5.6 Sol · AIHub");
    }

    #[test]
    fn aggregated_oauth_catalog_uses_owned_by_provider_suffix_and_keeps_provider_window() {
        let template = json!({"models":[{
            "slug":"gpt-5.6-luna",
            "display_name":"GPT-5.6-Luna",
            "context_window":272000,
            "max_context_window":272000
        }]});
        let metadata = MetadataResolver::empty();
        for model_id in ["gpt-5.6-luna", "gpt-5.6-luna-opencode-go"] {
            let models = vec![ModelInfo {
                id: model_id.to_owned(),
                display_name: Some("gpt-5.6-luna · OpenCode Go".to_owned()),
                owned_by: Some("opencode-go".to_owned()),
                context_window: Some(1_050_000),
                supports_image: Some(true),
                supports_thinking: Some(true),
                ..ModelInfo::default()
            }];
            let catalog = codex_oauth_proxy_catalog_from_aggregated_models_with_metadata(
                &models,
                1_000_000,
                Some(&template),
                &metadata,
            );
            assert_eq!(catalog["models"][1]["slug"], "gpt-5.6-luna-opencode-go");
            assert_eq!(catalog["models"][1][UPSTREAM_MODEL_MARKER], "gpt-5.6-luna");
            assert_eq!(catalog["models"][1]["context_window"], 1_050_000);
            assert_eq!(catalog["models"][1]["max_context_window"], 1_050_000);
            assert_eq!(
                catalog["models"][1]["input_modalities"],
                json!(["text", "image"])
            );
        }
    }

    #[test]
    fn oauth_proxy_catalog_keeps_smaller_upstream_gpt_context() {
        let template = json!({"models":[{
            "slug":"gpt-5.5",
            "context_window":272000,
            "max_context_window":272000
        }]});
        let models = vec![ModelInfo {
            id: "gpt-5.5".to_owned(),
            context_window: Some(200_000),
            ..ModelInfo::default()
        }];
        let catalog = codex_oauth_proxy_catalog_from_models(&models, 1_000_000, Some(&template));
        assert_eq!(catalog["models"][1]["context_window"], 200_000);
        assert_eq!(catalog["models"][1]["max_context_window"], 200_000);
    }

    #[test]
    fn refreshes_official_models_without_dropping_custom_models() {
        let current_official = json!({
            "client_version": "1.2.3",
            "etag": "catalog-etag",
            "models": [
                {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol","context_window":272000,"max_context_window":272000},
                {"slug":"gpt-5.5","display_name":"GPT-5.5"}
            ]
        });
        let managed = json!({
            "models": [
                {"slug":"gpt-5.5","display_name":"GPT-5.5"},
                {
                    "slug":"DeepSeek-V4-Flash",
                    "display_name":"DeepSeek-V4-Flash",
                    "description":"DeepSeek latest fast coding model | 0.2x | Value model",
                    "codex_mixin_managed":true,
                    "upgrade":{"model":"gpt-5.6-luna","migration_markdown":"Retired"},
                    "availability_nux":{"message":"Official model notice"},
                    "supports_search_tool":false,
                    "web_search_tool_type":"text_and_image"
                },
                {
                    "slug":"gpt-5.6-sol-custom",
                    "display_name":"gpt-5.5 (Custom)",
                    "description":"Custom upstream model exposed through codex-mixin",
                    "codex_mixin_upstream_model":"gpt-5.6-sol",
                    "context_window":372000,
                    "max_context_window":372000
                }
            ]
        });

        let mut refreshed = refresh_managed_oauth_catalog(&current_official, &managed).unwrap();
        apply_web_search_capabilities(&mut refreshed, &HashSet::new()).unwrap();
        let slugs = refreshed["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            slugs,
            vec![
                "gpt-5.6-sol",
                "gpt-5.5",
                "DeepSeek-V4-Flash",
                "gpt-5.6-sol-custom"
            ]
        );
        assert_eq!(refreshed["models"][2]["multi_agent_version"], "v2");
        assert_eq!(refreshed["models"][3]["multi_agent_version"], "v2");
        assert_eq!(
            refreshed["models"][2]["additional_speed_tiers"],
            json!(["fast"])
        );
        assert_eq!(refreshed["models"][2]["service_tiers"][0]["id"], "priority");
        assert_eq!(
            refreshed["models"][3]["additional_speed_tiers"],
            json!(["fast"])
        );
        // Provider-advertised windows are preserved; official GPT entries only fill gaps.
        assert_eq!(refreshed["models"][3]["context_window"], 372_000);
        assert_eq!(refreshed["models"][3]["max_context_window"], 372_000);
        assert!(refreshed["models"][2].get("web_search_tool_type").is_none());
        assert!(refreshed["models"][2].get("upgrade").is_none());
        assert!(refreshed["models"][2].get("availability_nux").is_none());
        for model in refreshed["models"].as_array().unwrap() {
            assert_eq!(model["base_instructions"], FALLBACK_BASE_INSTRUCTIONS);
            assert_eq!(
                model["model_messages"]["instructions_template"],
                FALLBACK_BASE_INSTRUCTIONS
            );
        }
        assert_eq!(refreshed["client_version"], "1.2.3");
        assert_eq!(refreshed["etag"], "catalog-etag");
    }

    #[test]
    fn rejects_custom_slug_collisions_during_refresh() {
        let official = json!({"models":[{"slug":"gpt-5.6-sol"}]});
        let managed = json!({"models":[{
            "slug":"gpt-5.6-sol",
            "description":"Custom upstream model exposed through codex-mixin"
        }]});
        let error = refresh_managed_oauth_catalog(&official, &managed).unwrap_err();
        assert!(error.to_string().contains("collides"));
    }
}
