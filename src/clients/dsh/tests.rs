use super::*;

#[test]
fn install_updates_live_profiles_and_survives_legacy_import() {
    let home = tempfile::tempdir().unwrap();
    let patch = home.path().join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::fs::write(&patch, "- id: llm-pi-ai\n  config:\n    providers:\n      other: {api: openai-completions}\n- id: unrelated\n  config: {enabled: true}\n").unwrap();
    let bind = "127.0.0.1:8787".parse().unwrap();
    assert!(install(home.path(), bind, vec![], "test-key").unwrap());
    let document = read_yaml(&patch, "test patch").unwrap();
    assert_eq!(
        document[0]["config"]["providers"][PROVIDER_ID]["baseURL"],
        "http://127.0.0.1:8787/v1"
    );
    assert_eq!(
        document[0]["config"]["providers"]["other"]["api"],
        "openai-completions"
    );
    assert_eq!(document[1]["id"], "unrelated");
    assert!(!install(home.path(), bind, vec![], "test-key").unwrap());
    std::fs::rename(
        home.path().join("settings.yaml"),
        home.path().join("settings.yaml.imported"),
    )
    .unwrap();
    assert!(is_managed(home.path()).unwrap());
    install(
        home.path(),
        "127.0.0.1:9797".parse().unwrap(),
        vec![],
        "rotated-key",
    )
    .unwrap();
    let document = read_yaml(&patch, "test patch").unwrap();
    assert_eq!(
        document[0]["config"]["providers"][PROVIDER_ID]["baseURL"],
        "http://127.0.0.1:9797/v1"
    );
    uninstall(home.path()).unwrap();
    assert!(!is_managed(home.path()).unwrap());
    let document = read_yaml(&patch, "test patch").unwrap();
    assert!(
        document[0]["config"]["providers"]
            .get(PROVIDER_ID)
            .is_none()
    );
    assert_eq!(
        document[0]["config"]["providers"]["other"]["api"],
        "openai-completions"
    );
    let imported = read_yaml(&home.path().join("settings.yaml.imported"), "test legacy").unwrap();
    assert!(
        imported["llm-pi-ai"]["providers"]
            .get(PROVIDER_ID)
            .is_none()
    );
}

#[test]
fn install_initializes_desktop_and_existing_cli_profiles() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("profiles/web")).unwrap();
    std::fs::create_dir_all(home.path().join("profiles/node_modules")).unwrap();
    install(
        home.path(),
        "127.0.0.1:8787".parse().unwrap(),
        vec![],
        "test-key",
    )
    .unwrap();
    for profile in ["desktop", "web"] {
        let patch = read_yaml(
            &home
                .path()
                .join(format!("profiles/{profile}/cordis.patch.yml")),
            "test patch",
        )
        .unwrap();
        assert_eq!(patch[0]["id"], "llm-pi-ai");
        assert_eq!(
            patch[0]["config"]["providers"][PROVIDER_ID]["displayName"],
            "Codex Mixin"
        );
    }
    assert!(
        !home
            .path()
            .join("profiles/node_modules/cordis.patch.yml")
            .exists()
    );
}

#[test]
fn imported_backup_alone_is_not_an_active_connection() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "127.0.0.1:8787".parse().unwrap(),
        vec![],
        "test-key",
    )
    .unwrap();
    std::fs::rename(
        home.path().join("settings.yaml"),
        home.path().join("settings.yaml.imported"),
    )
    .unwrap();
    let profiles = home.path().join("profiles");
    if profiles.exists() {
        std::fs::remove_dir_all(profiles).unwrap();
    }
    assert!(!is_managed(home.path()).unwrap());
}

#[test]
fn malformed_profile_does_not_change_settings_or_credentials() {
    let home = tempfile::tempdir().unwrap();
    let patch = home.path().join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::fs::write(&patch, "config: wrong-root\n").unwrap();
    let error = install(
        home.path(),
        "127.0.0.1:8787".parse().unwrap(),
        vec![],
        "test-key",
    )
    .unwrap_err();
    assert!(error.to_string().contains("YAML sequence"));
    assert!(!home.path().join("settings.yaml").exists());
    assert!(!home.path().join(".credentials.yaml").exists());
    assert_eq!(
        std::fs::read_to_string(&patch).unwrap(),
        "config: wrong-root\n"
    );
}

#[test]
fn profile_only_connection_disconnects_and_preserves_other_configuration() {
    let home = tempfile::tempdir().unwrap();
    let patch = home.path().join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::fs::write(&patch, "- id: llm-pi-ai\n  config:\n    providers:\n      codex-mixin: {displayName: 'Codex Mixin', apiKeyEnv: CODEX_MIXIN_GATEWAY_API_KEY}\n      other: {api: openai-completions}\n- id: unrelated\n  config: {enabled: true}\n").unwrap();
    assert!(is_managed(home.path()).unwrap());
    uninstall(home.path()).unwrap();
    assert!(!is_managed(home.path()).unwrap());
    let document = read_yaml(&patch, "test patch").unwrap();
    assert_eq!(
        document[0]["config"]["providers"]["other"]["api"],
        "openai-completions"
    );
    assert_eq!(document[1]["config"]["enabled"], true);
}

#[test]
fn profile_expressions_survive_provider_updates() {
    let home = tempfile::tempdir().unwrap();
    let patch = home.path().join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
    std::fs::write(
        &patch,
        "# Profile expressions must survive.\n---\n- id: unrelated\n  config:\n    root: !!js dshHomePath('sessions')\n    literal: '!!js is plain text'\n",
    )
    .unwrap();
    let before = read_yaml(&patch, "test patch").unwrap();
    install(
        home.path(),
        "127.0.0.1:8787".parse().unwrap(),
        vec![],
        "test-key",
    )
    .unwrap();
    let after = read_yaml(&patch, "test patch").unwrap();
    assert_eq!(before[0], after[0]);
    // DSH must still receive an expression node rather than a literal string.
    assert!(std::fs::read_to_string(&patch).unwrap().contains("js"));
    assert!(
        !install(
            home.path(),
            "127.0.0.1:8787".parse().unwrap(),
            vec![],
            "test-key"
        )
        .unwrap()
    );
    // Feed this real adapter output to DSH's own offline profile composer.
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/dsh-profile-fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::copy(&patch, fixtures.join(PROFILE_PATCH_FILE)).unwrap();
}
