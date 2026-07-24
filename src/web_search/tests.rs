use super::probe::*;
use super::storage::unix_seconds;
use super::types::{CAPABILITY_FILE_VERSION, CapabilitySnapshot, UpstreamIdentity};
use super::*;

fn test_config(upstream_base_url: &str) -> GatewayConfig {
    let mut provider = crate::provider::custom_provider("test-provider", "test-key");
    provider.base_url = upstream_base_url.to_owned();
    provider.selected_models = vec!["Claude Haiku 4.5".to_owned()];
    provider.cached_models = vec![crate::provider::ProviderModel {
        id: "Claude Haiku 4.5".to_owned(),
        ..crate::provider::ProviderModel::default()
    }];
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://example.test/responses".to_owned(),
        codex_auth_path: PathBuf::from("/tmp/auth.json"),
        gateway_api_key: None,
        accept_codex_oauth: false,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(30),
        thinking_mode: crate::config::ThinkingMode::Off,
        enable_web_search_tool: true,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

#[test]
fn recognizes_server_and_client_web_search_blocks() {
    let mut server = ProbeObservation::default();
    server.observe(&json!({
        "type": "content_block_start",
        "content_block": {"type":"server_tool_use","name":"web_search"}
    }));
    assert!(server.server_tool_started);
    assert!(!server.server_search_result);
    assert!(!server.ordinary_tool_call);

    server.observe(&json!({
        "type": "content_block_start",
        "content_block": {"type":"web_search_tool_result","tool_use_id":"srvtoolu_1"}
    }));
    assert!(server.server_search_result);

    let mut client = ProbeObservation::default();
    client.observe(&json!({
        "type": "content_block_start",
        "content_block": {"type":"tool_use","name":"web_search"}
    }));
    assert!(!client.server_tool_started);
    assert!(!client.server_search_result);
    assert!(client.ordinary_tool_call);
}

#[test]
fn verifies_flattened_release_answers() {
    assert!(response_matches_release(
        "The latest release is v0.144.5",
        "rust-v0.144.5"
    ));
    assert!(!response_matches_release(
        "The latest release is v0.114.0",
        "rust-v0.144.5"
    ));
}

#[test]
fn capability_annotation_preserves_provider_hint_until_probe_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("web-search-capabilities.json");
    let config = test_config("https://one.example");
    let capabilities = WebSearchCapabilities::load(path, &config).unwrap();
    let mut models = vec![ModelInfo {
        id: "Claude Haiku 4.5-test-provider".to_owned(),
        supports_web_search: Some(true),
        ..ModelInfo::default()
    }];

    capabilities.annotate_models(&mut models);

    assert_eq!(models[0].supports_web_search, Some(true));
}

#[tokio::test]
async fn persists_prunes_and_invalidates_capabilities() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("web-search-capabilities.json");
    let config = test_config("https://one.example");
    let capabilities = WebSearchCapabilities::load(path.clone(), &config).unwrap();
    let capability = |model: &str| ModelWebSearchCapability {
        model: model.to_owned(),
        provider_id: "test-provider".to_owned(),
        upstream_model: "Claude Haiku 4.5".to_owned(),
        supported: true,
        evidence: "server_tool_result".to_owned(),
        error: None,
        probed_at: unix_seconds().unwrap(),
    };
    {
        let mut models = capabilities.models.write().unwrap();
        models.insert(
            "Claude Haiku 4.5-test-provider".to_owned(),
            capability("Claude Haiku 4.5-test-provider"),
        );
        models.insert("Removed Model".to_owned(), capability("Removed Model"));
    }
    capabilities.save().unwrap();

    let loaded = WebSearchCapabilities::load(path.clone(), &config).unwrap();
    assert!(loaded.supports_model("Claude Haiku 4.5-test-provider"));
    assert!(loaded.supports_model("Removed Model"));
    let mut current_models = vec![ModelInfo {
        id: "Claude Haiku 4.5-test-provider".to_owned(),
        ..ModelInfo::default()
    }];
    let registry = ProviderRegistry::new(config.providers.clone()).unwrap();
    let summary = loaded
        .probe_models(&mut current_models, &config, &registry, false)
        .await
        .unwrap();
    assert_eq!(summary.attempted, 0);
    assert!(!loaded.supports_model("Removed Model"));

    let another_upstream = test_config("https://two.example");
    let invalidated = WebSearchCapabilities::load(path.clone(), &another_upstream).unwrap();
    assert!(!invalidated.supports_model("Claude Haiku 4.5-test-provider"));
    assert!(invalidated.results().is_empty());

    let old_snapshot = CapabilitySnapshot {
        version: CAPABILITY_FILE_VERSION - 1,
        upstream: UpstreamIdentity::from_config(&config),
        models: BTreeMap::from([(
            "Claude Haiku 4.5-test-provider".to_owned(),
            capability("Claude Haiku 4.5-test-provider"),
        )]),
    };
    fs::write(&path, serde_json::to_vec_pretty(&old_snapshot).unwrap()).unwrap();
    let invalidated = WebSearchCapabilities::load(path, &config).unwrap();
    assert!(invalidated.results().is_empty());
}
