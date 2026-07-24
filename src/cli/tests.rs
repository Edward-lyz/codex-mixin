use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use clap::Parser;
use toml_edit::DocumentMut;

use codex_mixin::anthropic::ModelInfo;
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::provider::{ProviderPreset, ProviderRegistry};
use codex_mixin::server::AppState;

use super::Cli;
use super::{atomic_file::*, codex::*, runtime::*, service::*, status::*};

#[test]
fn rotates_gateway_log_at_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gateway.log");
    fs::write(&log, b"12345").unwrap();

    rotate_gateway_log_if_needed(&log, 5).unwrap();

    assert!(!log.exists());
    assert_eq!(
        fs::read(dir.path().join("gateway.log.1")).unwrap(),
        b"12345"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(dir.path().join("gateway.log.1"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn keeps_gateway_log_below_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gateway.log");
    fs::write(&log, b"1234").unwrap();

    rotate_gateway_log_if_needed(&log, 5).unwrap();

    assert_eq!(fs::read(log).unwrap(), b"1234");
    assert!(!dir.path().join("gateway.log.1").exists());
}

#[test]
fn install_command_rejects_provider_override() {
    assert!(Cli::try_parse_from(["codex-mixin", "install-codex", "--provider", "custom"]).is_err());
}

#[test]
fn install_command_accepts_explicit_custom_only_mode() {
    assert!(Cli::try_parse_from(["codex-mixin", "install-codex", "--custom-only"]).is_ok());
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "install-codex",
            "--custom-only",
            "--codex-oauth-proxy",
        ])
        .is_err()
    );
}

#[test]
fn provider_select_accepts_an_empty_allowlist() {
    assert!(Cli::try_parse_from(["codex-mixin", "providers", "select", "provider-a"]).is_ok());
}

#[test]
fn macos_bridge_commands_accept_multi_provider_arguments() {
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "benchmark",
            "start",
            "--timeout-seconds",
            "10",
            "--provider",
            "provider-a",
            "--provider",
            "provider-b",
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "codex-mixin",
            "fusion",
            "set",
            "--profile-json",
            r#"{"id":"default","panel_models":["model-provider-a"],"judge_model":"model-provider-a","final_model":"model-provider-a"}"#,
            "--replace-id",
            "default",
        ])
        .is_ok()
    );
}

#[test]
fn oauth_proxy_catalog_uses_exact_aggregated_slugs() {
    let models = vec![ModelInfo {
        id: "gpt-5.6-sol-provider-with-hyphens".to_owned(),
        ..ModelInfo::default()
    }];

    assert!(model_exists_in_oauth_proxy_catalog(
        "gpt-5.6-sol-provider-with-hyphens",
        &models,
        None
    ));
    assert!(!model_exists_in_oauth_proxy_catalog(
        "gpt-5.6-sol-other-provider",
        &models,
        None
    ));
}

#[test]
fn oauth_proxy_install_supports_first_run_config_without_provider() {
    let mut doc = r#"
[projects."/Users/example/work"]
trust_level = "trusted"

[hooks.state]
"#
    .parse::<DocumentMut>()
    .unwrap();
    let catalog_path = PathBuf::from("/tmp/mixin-models.json");

    upsert_codex_config(
        &mut doc,
        None,
        &catalog_path,
        "http://127.0.0.1:8787/v1",
        "disabled",
        None,
        true,
    )
    .unwrap();

    assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
    assert_eq!(doc["web_search"].as_str(), Some("disabled"));
    assert_eq!(
        doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
        Some("http://127.0.0.1:8787/v1")
    );
    assert_eq!(
        doc["projects"]["/Users/example/work"]["trust_level"].as_str(),
        Some("trusted")
    );
}

#[test]
fn configures_web_search_without_changing_default_model() {
    let mut doc = DocumentMut::new();
    upsert_codex_config(
        &mut doc,
        None,
        Path::new("/tmp/mixin-models.json"),
        "http://127.0.0.1:8787/v1",
        "live",
        None,
        true,
    )
    .unwrap();

    assert_eq!(doc["web_search"].as_str(), Some("live"));
    assert!(doc.get("model").is_none());
}

#[test]
fn custom_config_controls_default_catalog_and_models_cache_paths() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("managed-codex").join("config.toml");

    let paths = resolve_codex_install_paths(Some(config_path.clone()), None).unwrap();

    assert_eq!(paths.config, config_path);
    assert_eq!(
        paths.catalog,
        dir.path()
            .join("managed-codex")
            .join("model-catalogs")
            .join("mixin-models.json")
    );
    assert_eq!(
        paths.models_cache,
        dir.path().join("managed-codex").join("models_cache.json")
    );
}

#[test]
fn explicit_relative_config_and_catalog_paths_become_absolute() {
    let relative_config = PathBuf::from("target/codex-mixin-test/config.toml");
    let relative_catalog = PathBuf::from("target/codex-mixin-test/catalog.json");

    let paths = resolve_codex_install_paths(
        Some(relative_config.clone()),
        Some(relative_catalog.clone()),
    )
    .unwrap();

    assert_eq!(paths.config, std::path::absolute(relative_config).unwrap());
    assert_eq!(
        paths.catalog,
        std::path::absolute(relative_catalog).unwrap()
    );
    assert!(paths.models_cache.is_absolute());
}

#[test]
fn oauth_install_missing_cache_creates_no_restore_marker_or_directory() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("managed-codex").join("config.toml");
    let paths = resolve_codex_install_paths(Some(config_path.clone()), None).unwrap();

    let error = load_codex_install_template(&paths, true).unwrap_err();

    assert!(error.to_string().contains("model cache is missing"));
    assert!(!config_path.parent().unwrap().exists());
    assert!(!managed_backup_path(&config_path).exists());
    assert!(!managed_absent_marker_path(&config_path).exists());
}

#[test]
fn custom_only_install_ignores_models_cache() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("managed-codex").join("config.toml");
    let paths = resolve_codex_install_paths(Some(config_path.clone()), None).unwrap();
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(&paths.models_cache, "not valid JSON").unwrap();

    assert_eq!(load_codex_install_template(&paths, false).unwrap(), None);
}

#[tokio::test]
async fn oauth_install_falls_back_to_local_cache_when_official_fetch_fails() {
    let upstream = axum::Router::new().route(
        "/backend-api/codex/models",
        axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let paths = resolve_codex_install_paths(Some(config_path), None).unwrap();
    fs::write(
        &paths.models_cache,
        r#"{"client_version":"0.144.0","models":[{"slug":"gpt-5.6-sol","context_window":272000}]}"#,
    )
    .unwrap();
    let auth_path = dir.path().join("auth.json");
    fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"secret","account_id":"account-one"}}"#,
    )
    .unwrap();
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![codex_mixin::provider::open_code_go_provider(
            "test-provider",
            "upstream-key",
        )],
        official_responses_url: format!("http://{address}/backend-api/codex/responses"),
        codex_auth_path: auth_path,
        gateway_api_key: None,
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();

    let template = load_codex_install_template_online(&paths, true, &state)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(template["models"][0]["slug"], "gpt-5.6-sol");
    assert_eq!(template["models"][0]["context_window"], 272_000);
}

#[test]
fn custom_only_provider_does_not_require_openai_auth() {
    let mut doc = DocumentMut::new();

    upsert_codex_config(
        &mut doc,
        Some("DeepSeek-V4-Flash"),
        Path::new("/tmp/mixin-models.json"),
        "http://127.0.0.1:8787/v1",
        "live",
        None,
        false,
    )
    .unwrap();

    assert_eq!(doc["model"].as_str(), Some("DeepSeek-V4-Flash"));
    let provider = doc["model_providers"]["codex-mixin"].as_table().unwrap();
    assert!(provider.get("requires_openai_auth").is_none());
    assert!(provider.get("supports_websockets").is_none());
}

#[test]
fn managed_install_backup_and_uninstall_restore_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");
    fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
    let original_config = "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\n";
    fs::write(&config_path, original_config).unwrap();
    fs::write(&catalog_path, "{}").unwrap();
    let session_path = dir.path().join("sessions/legacy.jsonl");
    fs::create_dir_all(session_path.parent().unwrap()).unwrap();
    fs::write(
        &session_path,
        r#"{"type":"session_meta","payload":{"model_provider":"codex-mixin"}}"#,
    )
    .unwrap();

    let original = read_managed_config_for_install(&config_path).unwrap();
    assert_eq!(original, original_config);
    create_managed_config_restore_point(&config_path, &original).unwrap();
    assert!(managed_backup_path(&config_path).exists());
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel = \"Claude Sonnet 5\"\nmodel_catalog_json = {:?}\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();

    uninstall_codex(Some(config_path.clone()), None).unwrap();
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original_config);
    assert!(
        fs::read_to_string(&session_path)
            .unwrap()
            .contains(r#""model_provider":"custom""#)
    );
    assert!(!managed_backup_path(&config_path).exists());
    assert!(!catalog_path.exists());
}

#[test]
fn failed_codex_validation_rolls_back_config_catalog_and_restore_point() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let catalog_path = dir.path().join("mixin-models.json");
    let original_config = b"model_provider = \"openai\"\n";
    let original_catalog = b"{\"models\":[{\"slug\":\"original\"}]}";
    fs::write(&config_path, original_config).unwrap();
    fs::write(&catalog_path, original_catalog).unwrap();
    let paths = CodexInstallPaths {
        config: config_path.clone(),
        catalog: catalog_path.clone(),
        models_cache: dir.path().join("models_cache.json"),
    };

    let error = write_managed_codex_files(
        &paths,
        std::str::from_utf8(original_config).unwrap(),
        b"{\"models\":[{\"slug\":\"custom\"}]}",
        format!("{MANAGED_CONFIG_HEADER}\nmodel_provider = \"codex-mixin\"\n").as_bytes(),
        || anyhow::bail!("validator rejected candidate"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("installation rolled back"));
    assert_eq!(fs::read(&config_path).unwrap(), original_config);
    assert_eq!(fs::read(&catalog_path).unwrap(), original_catalog);
    assert!(!managed_backup_path(&config_path).exists());
    assert!(!managed_absent_marker_path(&config_path).exists());
}

#[test]
fn managed_uninstall_removes_config_when_none_existed_before() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");

    let original = read_managed_config_for_install(&config_path).unwrap();
    assert!(original.is_empty());
    create_managed_config_restore_point(&config_path, &original).unwrap();
    assert!(managed_absent_marker_path(&config_path).exists());
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    let session_path = dir.path().join("sessions/first-run.jsonl");
    fs::create_dir_all(session_path.parent().unwrap()).unwrap();
    fs::write(
        &session_path,
        r#"{"type":"session_meta","payload":{"model_provider":"codex-mixin"}}"#,
    )
    .unwrap();

    uninstall_codex(Some(config_path.clone()), Some(catalog_path)).unwrap();
    assert!(!config_path.exists());
    assert!(!managed_absent_marker_path(&config_path).exists());
    assert!(
        fs::read_to_string(session_path)
            .unwrap()
            .contains(r#""model_provider":"openai""#)
    );
}

#[test]
fn oauth_proxy_install_replaces_legacy_custom_provider() {
    let mut doc = r#"
model_provider = "custom"
model = "gpt-5.5"

[model_providers.custom]
name = "OpenAI"
requires_openai_auth = true
supports_websockets = true
wire_api = "responses"
"#
    .parse::<DocumentMut>()
    .unwrap();
    let catalog_path = PathBuf::from("/tmp/mixin-models.json");

    upsert_codex_config(
        &mut doc,
        None,
        &catalog_path,
        "http://127.0.0.1:8787/v1",
        "disabled",
        None,
        true,
    )
    .unwrap();

    assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
    assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(
        doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
        Some("http://127.0.0.1:8787/v1")
    );
    assert_eq!(
        doc["model_providers"]["codex-mixin"]["requires_openai_auth"].as_bool(),
        Some(true)
    );
    assert_eq!(
        doc["model_providers"]["custom"]["wire_api"].as_str(),
        Some("responses")
    );
}

#[test]
fn oauth_proxy_install_replaces_conflicting_mixin_provider_table() {
    let mut doc = r#"
[model_providers.codex-mixin]
name = "stale"
base_url = "https://stale.example/v1"
env_key = "STALE_KEY"
experimental_bearer_token = "stale-token"
custom_field = "stale"
"#
    .parse::<DocumentMut>()
    .unwrap();
    let catalog_path = PathBuf::from("/tmp/mixin-models.json");

    upsert_codex_config(
        &mut doc,
        None,
        &catalog_path,
        "http://127.0.0.1:8787/v1",
        "disabled",
        None,
        true,
    )
    .unwrap();

    let provider = doc["model_providers"]["codex-mixin"].as_table().unwrap();
    assert_eq!(provider["name"].as_str(), Some("Codex Mixin"));
    assert_eq!(
        provider["base_url"].as_str(),
        Some("http://127.0.0.1:8787/v1")
    );
    assert_eq!(provider["requires_openai_auth"].as_bool(), Some(true));
    assert!(provider.get("env_key").is_none());
    assert!(provider.get("experimental_bearer_token").is_none());
    assert!(provider.get("custom_field").is_none());
}

#[test]
fn oauth_proxy_install_writes_codex_mixin_provider_without_default_model() {
    let mut doc = "model = \"gpt-5.5\"\n".parse::<DocumentMut>().unwrap();
    let catalog_path = PathBuf::from("/tmp/mixin-models.json");

    upsert_codex_config(
        &mut doc,
        None,
        &catalog_path,
        "http://127.0.0.1:8787/v1",
        "disabled",
        None,
        true,
    )
    .unwrap();

    assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
    assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(
        doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
        Some("http://127.0.0.1:8787/v1")
    );
}

#[test]
fn refreshes_managed_catalog_from_latest_official_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let official_path = dir.path().join("models_cache.json");
    let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");
    fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nrequires_openai_auth = true\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        &official_path,
        r#"{"models":[{"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol"},{"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra"},{"slug":"gpt-5.6-luna","display_name":"GPT-5.6-Luna"}]}"#,
    )
    .unwrap();
    fs::write(
        &catalog_path,
        r#"{"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5"},{"slug":"DeepSeek-V4-Flash","description":"Custom upstream model exposed through codex-mixin"}]}"#,
    )
    .unwrap();

    assert!(refresh_managed_codex_catalog(&config_path).unwrap());
    let refreshed: serde_json::Value =
        serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
    assert_eq!(refreshed["models"][0]["slug"], "gpt-5.6-sol");
    assert_eq!(refreshed["models"][1]["slug"], "gpt-5.6-terra");
    assert_eq!(refreshed["models"][2]["slug"], "gpt-5.6-luna");
    assert_eq!(refreshed["models"][3]["slug"], "DeepSeek-V4-Flash");
    assert_eq!(refreshed["models"][3]["multi_agent_version"], "v2");
    for model in refreshed["models"].as_array().unwrap() {
        assert!(model["base_instructions"].is_string());
        assert!(model["model_messages"]["instructions_template"].is_string());
    }
    assert!(!refresh_managed_codex_catalog(&config_path).unwrap());
}

#[test]
fn capability_refresh_does_not_restore_stale_official_cache() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let official_path = dir.path().join("models_cache.json");
    let catalog_path = dir.path().join("mixin-models.json");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nrequires_openai_auth = true\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        official_path,
        r#"{"models":[{"slug":"gpt-5.6-sol","context_window":372000}]}"#,
    )
    .unwrap();
    fs::write(
        &catalog_path,
        r#"{"models":[{"slug":"gpt-5.6-sol","context_window":272000},{"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true}]}"#,
    )
    .unwrap();

    refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&HashSet::new())).unwrap();

    let refreshed: serde_json::Value =
        serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
    assert_eq!(refreshed["models"][0]["context_window"], 272_000);
}

#[test]
fn parses_installed_codex_client_version() {
    assert_eq!(
        parse_codex_client_version("codex-cli 0.144.4\n").as_deref(),
        Some("0.144.4")
    );
    assert_eq!(parse_codex_client_version("codex-cli unknown"), None);
}

#[test]
fn non_oauth_managed_config_skips_oauth_catalog_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nwire_api = \"responses\"\n",
            dir.path().join("mixin-models.json").to_string_lossy()
        ),
    )
    .unwrap();

    assert!(!refresh_managed_codex_catalog(&config_path).unwrap());
}

#[test]
fn refreshes_per_model_web_search_for_non_oauth_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let catalog_path = dir.path().join("mixin-models.json");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nwire_api = \"responses\"\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        &catalog_path,
        r#"{"models":[{"slug":"Claude Haiku 4.5","codex_mixin_managed":true},{"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true,"web_search_tool_type":"text"}]}"#,
    )
    .unwrap();

    let supported_models = HashSet::from(["Claude Haiku 4.5".to_owned()]);
    assert!(
        refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models))
            .unwrap()
    );
    let refreshed: serde_json::Value =
        serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
    assert_eq!(refreshed["models"][0]["web_search_tool_type"], "text");
    assert!(refreshed["models"][1].get("web_search_tool_type").is_none());
}

#[test]
fn generated_catalog_refresh_adds_new_fusion_models() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let catalog_path = dir.path().join("mixin-models.json");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nrequires_openai_auth = true\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        &catalog_path,
        r#"{"models":[{"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true}]}"#,
    )
    .unwrap();
    let generated = serde_json::json!({
        "models": [
            {"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true},
            {"slug":"mixin/fusion/default","display_name":"Fusion (default)","codex_mixin_managed":true}
        ]
    });

    assert!(
        write_generated_managed_codex_catalog(&config_path, generated, &HashSet::new()).unwrap()
    );
    let refreshed: serde_json::Value =
        serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
    assert!(
        refreshed["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| { model["slug"] == "mixin/fusion/default" })
    );
}

#[test]
fn uninstall_rejects_catalog_that_differs_from_managed_config() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let managed_catalog_path = dir.path().join("managed-models.json");
    let explicit_catalog_path = dir.path().join("other-models.json");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n",
            managed_catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        managed_backup_path(&config_path),
        "model_provider = \"openai\"\n",
    )
    .unwrap();
    fs::write(&managed_catalog_path, "{}").unwrap();
    fs::write(&explicit_catalog_path, "{}").unwrap();

    let error = uninstall_codex(
        Some(config_path.clone()),
        Some(explicit_catalog_path.clone()),
    )
    .unwrap_err();

    assert!(error.to_string().contains("does not match"));
    assert!(is_managed_config(&fs::read_to_string(config_path).unwrap()));
    assert!(managed_catalog_path.exists());
    assert!(explicit_catalog_path.exists());
}

#[test]
fn summarizes_generic_quota_shapes() {
    assert_eq!(
        summarize_quota_json(&serde_json::json!({"usage":{"used":"12.5","budget":100}})),
        "quota used: 12.5 / 100"
    );
    assert_eq!(
        summarize_quota_json(&serde_json::json!({"data":{"used":42}})),
        "quota used: 42"
    );
    assert_eq!(
        summarize_quota_json(
            &serde_json::json!({"data":{"used_quota":10,"month_quota_limit":50,"remaining_quota":40}})
        ),
        "quota used: 10 / 50, remaining: 40"
    );
}

#[test]
fn preserves_quota_limit_and_remaining_for_visualization() {
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::BaiduOneApi,
            &serde_json::json!({
                "data": {
                    "used_quota": 10,
                    "month_quota_limit": 50,
                    "remaining_quota": 40
                }
            })
        )
        .unwrap(),
        QuotaUsageSummary {
            used: 10.0,
            limit: Some(50.0),
            remaining: Some(40.0),
        }
    );
    assert_eq!(
        quota_usage(
            codex_mixin::provider::ProviderQuotaParser::OpenRouter,
            &serde_json::json!({"data":{"total_usage":12.5,"total_credits":100}})
        )
        .unwrap(),
        QuotaUsageSummary {
            used: 12.5,
            limit: Some(100.0),
            remaining: Some(87.5),
        }
    );
}

#[test]
fn provider_presets_resolve_quota_urls() {
    let mut baidu = ProviderPreset::BaiduOneApi.create("baidu", "key");
    baidu.base_url = "https://oneapi.example".to_owned();
    baidu.quota_url = Some("https://oneapi.example/openapi/v3/user/quota".to_owned());
    baidu.quota_username = Some("quota-user".to_owned());
    let registry = ProviderRegistry::new(vec![baidu]).unwrap();
    assert_eq!(
        registry
            .provider("baidu")
            .unwrap()
            .quota_url()
            .unwrap()
            .as_str(),
        "https://oneapi.example/openapi/v3/user/quota?username=quota-user"
    );

    let openrouter = ProviderPreset::OpenRouter.create("openrouter", "key");
    let registry = ProviderRegistry::new(vec![openrouter]).unwrap();
    assert_eq!(
        registry
            .provider("openrouter")
            .unwrap()
            .quota_url()
            .unwrap()
            .as_str(),
        "https://openrouter.ai/api/v1/credits"
    );

    let deepseek = ProviderPreset::DeepSeek.create("deepseek", "key");
    let registry = ProviderRegistry::new(vec![deepseek]).unwrap();
    assert!(registry.provider("deepseek").unwrap().quota_url().is_none());
}

#[tokio::test]
async fn automatic_bind_uses_an_available_loopback_port() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_bind = occupied.local_addr().unwrap();

    let automatic = bind_gateway_listener(occupied_bind, true).await.unwrap();
    assert_ne!(automatic.local_addr().unwrap(), occupied_bind);

    let explicit = bind_gateway_listener(occupied_bind, false)
        .await
        .unwrap_err();
    assert_eq!(
        explicit.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::AddrInUse
    );
}

#[test]
fn outdated_gateway_runtime_is_replaced_on_its_existing_bind() {
    let legacy_runtime: RuntimeMetadata =
        serde_json::from_str(r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1}"#).unwrap();
    let older_runtime: RuntimeMetadata = serde_json::from_str(
        r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1,"version":"0.2.15"}"#,
    )
    .unwrap();
    let current_runtime: RuntimeMetadata = serde_json::from_value(serde_json::json!({
        "pid": 42,
        "bind": "127.0.0.1:18787",
        "started_at": 1,
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .unwrap();
    let existing_bind = "127.0.0.1:18787".parse().unwrap();

    assert_eq!(
        replacement_bind_for_outdated_runtime(&legacy_runtime, env!("CARGO_PKG_VERSION")),
        Some(existing_bind)
    );
    assert_eq!(
        replacement_bind_for_outdated_runtime(&older_runtime, env!("CARGO_PKG_VERSION")),
        Some(existing_bind)
    );
    assert_eq!(
        replacement_bind_for_outdated_runtime(&current_runtime, env!("CARGO_PKG_VERSION")),
        None
    );
}

#[test]
fn syncs_dynamic_gateway_port_to_managed_codex_provider() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            "{MANAGED_CONFIG_HEADER}\n\n[model_providers.codex-mixin]\nbase_url = \"http://127.0.0.1:8787/v1\"\nwire_api = \"responses\"\n\n[model_providers.other]\nbase_url = \"https://example.test/v1\"\n"
        ),
    )
    .unwrap();

    assert!(
        sync_managed_codex_gateway_base_url(&config_path, "127.0.0.1:18787".parse().unwrap())
            .unwrap()
    );
    let doc = fs::read_to_string(&config_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
        Some("http://127.0.0.1:18787/v1")
    );
    assert_eq!(
        doc["model_providers"]["other"]["base_url"].as_str(),
        Some("https://example.test/v1")
    );
}

#[cfg(unix)]
#[test]
fn atomic_rewrite_preserves_existing_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(write_atomic_if_changed(&path, b"new").unwrap());

    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
