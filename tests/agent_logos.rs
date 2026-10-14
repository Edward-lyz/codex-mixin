use codex_mixin::application::agent_versions::AGENT_VERSION_IDS;
use serde_json::Value;
use std::path::Path;

#[test]
fn every_fixed_agent_has_a_bundled_brand_asset() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("macos/assets/agents");
    let manifest: Value =
        serde_json::from_str(include_str!("../macos/assets/agents/manifest.json")).unwrap();
    assert_eq!(manifest.as_object().unwrap().len(), AGENT_VERSION_IDS.len());
    for id in AGENT_VERSION_IDS {
        let asset = &manifest[id];
        let file = asset["file"]
            .as_str()
            .unwrap_or_else(|| panic!("{id} has no logo"));
        assert_eq!(Path::new(file).file_stem().unwrap(), id);
        assert!(asset["template"].is_boolean());
        let bytes = std::fs::read(root.join(file)).unwrap();
        if file.ends_with(".png") {
            let image = image::load_from_memory(&bytes).unwrap();
            assert!(image.width() > 0 && image.height() > 0, "{id}");
            assert_eq!(asset["template"], false);
        } else {
            assert!(file.ends_with(".svg"));
            let svg = std::str::from_utf8(&bytes).unwrap();
            assert!(svg.contains("<svg") && svg.contains("</svg>"), "{id}");
        }
    }
}
