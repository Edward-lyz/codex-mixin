mod common;

use codex_mixin::provider::ProviderPreset;
use serde_json::Value;

#[test]
fn interface_exposes_provider_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let output = common::cli(directory.path())
        .args(["--no-tui", "interface", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let interface: Value = serde_json::from_slice(&output.stdout).unwrap();
    let catalog = &interface["provider_catalog"];
    let presets = catalog["presets"]
        .as_array()
        .expect("CLI must describe its provider presets");
    assert_eq!(presets.len(), ProviderPreset::ALL.len());
    for preset in ProviderPreset::ALL {
        let entry = presets
            .iter()
            .find(|entry| entry["id"] == preset.as_str())
            .unwrap();
        assert_eq!(entry["display_name"], preset.spec().display_name);
        assert!(entry["api_key_required"].is_boolean());
        assert!(entry["custom_endpoint"].is_boolean());
        assert!(entry["quota_username_required"].is_boolean());
        assert!(entry["quota_credentials_required"].is_boolean());
    }
    let agy = presets.iter().find(|entry| entry["id"] == "agy").unwrap();
    assert_eq!(agy["auth_mode"], "local_oauth");
    assert_eq!(agy["api_key_required"], false);
    let protocols = catalog["protocols"].as_array().unwrap();
    assert!(protocols.iter().any(|entry| entry["id"] == "google_gemini"));
    assert!(
        presets
            .iter()
            .any(|entry| entry["id"] == catalog["default_preset"])
    );
}
