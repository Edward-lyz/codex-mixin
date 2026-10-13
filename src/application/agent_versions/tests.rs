use super::*;

#[test]
fn parses_versions_from_cli_output() {
    assert_eq!(parse_version("codex-cli 0.42.0"), Some("0.42.0".to_owned()));
    assert_eq!(
        parse_version("2.1.3 (Claude Code)"),
        Some("2.1.3".to_owned())
    );
    assert_eq!(
        parse_version("\u{1b}[32mv1.2.3\u{1b}[0m"),
        Some("1.2.3".to_owned())
    );
    assert_eq!(
        parse_version("1.0.0-beta.2\n"),
        Some("1.0.0-beta.2".to_owned())
    );
    assert_eq!(parse_version("no version here"), None);
    assert_eq!(parse_version(""), None);
}

#[test]
fn orders_releases_after_prereleases() {
    assert!(newer("0.42.1", "0.42.0"));
    assert!(newer("2.0", "1.9.9"));
    assert!(newer("v1.0.0", "0.9.9"));
    assert!(!newer("0.42.0", "0.42.0"));
    assert!(newer("1.0.87", "1.0.87-0"));
    assert!(!newer("1.0.0-0", "1.0.0"));
    assert!(newer("1.0.0-2", "1.0.0-10"));
    assert!(newer("1.0.0-beta.2", "1.0.0-beta.1"));
    assert!(!newer("1.0.0-beta.1", "1.0.0-beta.2"));
}

#[test]
fn a_failing_version_probe_keeps_the_path_and_reports_the_error() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join(platform::executable_file_name("dsh"));
    std::fs::write(&bin, b"not a program").unwrap();
    let spec = spec_for("dsh").expect("dsh spec");
    let state = inspect_cli(spec, bin.clone(), None);
    let expected = bin.to_string_lossy().into_owned();
    assert_eq!(state.info.path.as_deref(), Some(expected.as_str()));
    assert_eq!(state.info.version, None);
    let error = state.info.error.expect("the probe failure is reported");
    assert!(error.contains(&expected), "{error}");
    assert!(
        state.updater.is_none(),
        "an unknown source offers no update"
    );
}

#[test]
fn recognizes_an_unknown_install_source() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("tools").join("dsh");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    assert!(detect_updater(spec_for("dsh").expect("dsh spec"), &bin).is_none());
}

#[test]
fn recognizes_a_vendor_self_update_install() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join(".local/share/claude/versions/2.1.3");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let updater = detect_updater(spec_for("claude").expect("claude spec"), &bin)
        .expect("the vendor installer's own updater");
    assert_eq!(updater.via, "self");
    assert_eq!(updater.args, vec!["update".to_owned()]);
    assert_eq!(
        updater.latest,
        LatestSource::Npm("@anthropic-ai/claude-code".to_owned())
    );
    assert!(updater.display_command().ends_with(" update"));
}

#[test]
fn recognizes_an_opencode_home_install() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join(".opencode/bin/opencode");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let updater =
        detect_updater(spec_for("opencode").expect("opencode spec"), &bin).expect("self updater");
    assert_eq!(updater.via, "self");
    assert_eq!(updater.args, vec!["upgrade".to_owned()]);
}

#[test]
fn recognizes_a_homebrew_install() {
    let directory = tempfile::tempdir().unwrap();
    let prefix = directory.path().join("homebrew");
    let bin = prefix.join("Cellar/codex/0.42.0/bin/codex");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let brew = prefix.join("bin").join("brew");
    std::fs::create_dir_all(brew.parent().unwrap()).unwrap();
    std::fs::write(&brew, b"").unwrap();
    let updater =
        detect_updater(spec_for("codex").expect("codex spec"), &bin).expect("brew updater");
    assert_eq!(updater.via, "brew");
    assert_eq!(updater.display_command(), "brew upgrade codex");
    assert_eq!(
        updater.latest,
        LatestSource::Brew {
            package: "codex".to_owned(),
            cask: false,
        }
    );
}

#[test]
fn a_homebrew_formula_of_another_agent_is_not_this_cli() {
    let directory = tempfile::tempdir().unwrap();
    let prefix = directory.path().join("homebrew");
    let bin = prefix.join("Cellar/pi-coding-agent/0.5.0/bin/pi");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let brew = prefix.join("bin").join("brew");
    std::fs::create_dir_all(brew.parent().unwrap()).unwrap();
    std::fs::write(&brew, b"").unwrap();
    // The formula belongs to Pi, not to Codex, which shares no formula.
    assert!(detect_updater(spec_for("codex").expect("codex spec"), &bin).is_none());
}

#[cfg(not(windows))]
#[test]
fn recognizes_an_npm_global_install() {
    let directory = tempfile::tempdir().unwrap();
    // Canonical: the updater reports the path its links lead to, which on
    // macOS resolves /var to /private/var.
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let bin = root.join("lib/node_modules/@openai/codex/bin/codex");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let npm = root.join("bin").join("npm");
    std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
    std::fs::write(&npm, b"").unwrap();
    let updater =
        detect_updater(spec_for("codex").expect("codex spec"), &bin).expect("npm updater");
    assert_eq!(updater.via, "npm");
    assert_eq!(updater.program, npm);
    assert_eq!(
        updater.args,
        vec![
            "install".to_owned(),
            "-g".to_owned(),
            "--prefix".to_owned(),
            root.to_string_lossy().into_owned(),
            "@openai/codex@latest".to_owned(),
        ]
    );
    assert!(updater.blocked.is_none(), "a temp folder is writable");
    assert!(
        updater
            .display_command()
            .starts_with("npm install -g --prefix ")
    );
}

#[cfg(windows)]
#[test]
fn recognizes_an_npm_global_install() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let bin = root.join("node_modules/@openai/codex/bin/codex.js");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    let npm = root.join("npm.cmd");
    std::fs::write(&npm, b"").unwrap();
    let updater =
        detect_updater(spec_for("codex").expect("codex spec"), &bin).expect("npm updater");
    assert_eq!(updater.via, "npm");
    assert_eq!(
        platform::agent_cli::real_executable(&updater.program),
        platform::agent_cli::real_executable(&npm)
    );
    assert!(updater.args.contains(&"@openai/codex@latest".to_owned()));
}

#[test]
fn a_shim_script_names_the_node_modules_folder() {
    let directory = tempfile::tempdir().unwrap();
    let package = "@openai/codex";
    let modules = directory.path().join("node_modules");
    std::fs::create_dir_all(modules.join(package)).unwrap();
    let bin = directory.path().join("codex.cmd");
    std::fs::write(
        &bin,
        format!("%dp0%\\node_modules\\{package}\\bin\\codex.js"),
    )
    .unwrap();
    assert_eq!(shim_modules(&bin, package), Some(modules));
    assert_eq!(shim_modules(&bin, "@other/cli"), None);
}

#[test]
fn a_version_manager_of_its_own_is_not_updated() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory
        .path()
        .join(".volta/tools/image/node/22/lib/node_modules/@openai/codex/bin/codex");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"").unwrap();
    assert!(detect_updater(spec_for("codex").expect("codex spec"), &bin).is_none());
}

#[test]
fn an_unknown_latest_version_is_not_up_to_date() {
    let mut info = AgentCliInfo {
        version: Some("1.0.0".to_owned()),
        ..AgentCliInfo::default()
    };
    apply_latest(
        &mut info,
        &npm_updater_for_test(),
        CachedAnswer {
            value: None,
            error: Some("query npm for @openai/codex: connection refused".to_owned()),
        },
    );
    assert_eq!(info.latest, None);
    assert!(!info.update, "an unknown newest version is not up to date");
    assert_eq!(
        info.error.as_deref(),
        Some("query npm for @openai/codex: connection refused")
    );
}

#[test]
fn a_newer_latest_marks_the_update() {
    let mut info = AgentCliInfo {
        version: Some("1.0.0".to_owned()),
        ..AgentCliInfo::default()
    };
    apply_latest(
        &mut info,
        &npm_updater_for_test(),
        CachedAnswer {
            value: Some("1.2.0".to_owned()),
            error: None,
        },
    );
    assert_eq!(info.latest.as_deref(), Some("1.2.0"));
    assert!(info.update);
    assert_eq!(info.error, None);
}

#[tokio::test]
async fn a_failing_latest_lookup_is_reported_and_not_up_to_date() {
    let directory = tempfile::tempdir().unwrap();
    let package = "codex-agent-versions-test";
    let updater = Updater {
        via: "brew",
        program: directory.path().join("missing-brew"),
        args: vec!["upgrade".to_owned(), package.to_owned()],
        latest: LatestSource::Brew {
            package: package.to_owned(),
            cask: false,
        },
        blocked: None,
    };
    let answer = latest_version(&updater).await;
    assert_eq!(answer.value, None);
    let error = answer.error.expect("the failed lookup is reported");
    assert!(error.contains("missing-brew"), "{error}");

    let mut info = AgentCliInfo {
        version: Some("1.0.0".to_owned()),
        ..AgentCliInfo::default()
    };
    apply_latest(&mut info, &updater, answer);
    assert_eq!(info.latest, None);
    assert!(
        !info.update,
        "a failed newest-version lookup is not up to date"
    );
    assert!(info.error.is_some(), "the failure stays in the row");
}

#[test]
fn a_blocked_install_offers_no_update() {
    let mut info = AgentCliInfo {
        version: Some("1.0.0".to_owned()),
        ..AgentCliInfo::default()
    };
    let mut updater = npm_updater_for_test();
    updater.blocked = Some("npm's global folder needs an administrator".to_owned());
    apply_latest(
        &mut info,
        &updater,
        CachedAnswer {
            value: Some("2.0.0".to_owned()),
            error: None,
        },
    );
    assert_eq!(info.latest.as_deref(), Some("2.0.0"));
    assert!(!info.update);
}

#[test]
fn errors_keep_their_earlier_context() {
    let mut info = AgentCliInfo::default();
    push_error(&mut info, "first".to_owned());
    push_error(&mut info, "second".to_owned());
    assert_eq!(info.error.as_deref(), Some("first; second"));
}

#[test]
fn reads_the_version_from_homebrew_info() {
    let formula = r#"{"formulae":[{"name":"codex","versions":{"stable":"0.42.1"}}],"casks":[]}"#;
    assert_eq!(brew_info_version(formula, "codex").unwrap(), "0.42.1");
    let cask = r#"{"formulae":[],"casks":[{"token":"claude","version":"0.11.6,build1"}]}"#;
    assert_eq!(brew_info_version(cask, "claude").unwrap(), "0.11.6");
    assert!(brew_info_version(r#"{"formulae":[],"casks":[]}"#, "codex").is_err());
}

#[test]
fn quotes_arguments_that_contain_spaces() {
    let updater = Updater {
        via: "npm",
        program: PathBuf::from("/usr/local/bin/npm"),
        args: vec!["install".to_owned(), "a b".to_owned()],
        latest: LatestSource::Npm("@openai/codex".to_owned()),
        blocked: None,
    };
    assert_eq!(updater.display_command(), "npm install 'a b'");
}

#[test]
fn caches_answers_per_key_and_stamp() {
    let cache = Cache::default();
    let answer = CachedAnswer {
        value: Some("1.0.0".to_owned()),
        error: None,
    };
    assert_eq!(cache.get("version:bin", "stamp"), None);
    cache.store("version:bin", "stamp", answer.clone(), VERSION_CACHE_OK);
    assert_eq!(cache.get("version:bin", "stamp"), Some(answer.clone()));
    assert_eq!(cache.get("version:bin", "other"), None);
    assert_eq!(cache.last("version:bin", "other"), None);
    cache.store(
        "version:bin",
        "stamp",
        CachedAnswer {
            value: Some("1.0.0".to_owned()),
            error: Some("probe failed".to_owned()),
        },
        Duration::ZERO,
    );
    assert_eq!(cache.get("version:bin", "stamp"), None);
    assert_eq!(
        cache
            .last("version:bin", "stamp")
            .and_then(|answer| answer.value),
        Some("1.0.0".to_owned()),
        "a failed ask keeps the version it last knew"
    );
    cache.forget("version:bin");
    assert_eq!(cache.last("version:bin", "stamp"), None);
}

#[test]
fn every_listed_agent_has_a_spec() {
    for id in AGENT_VERSION_IDS {
        assert!(spec_for(id).is_some(), "{id}");
    }
    assert!(spec_for("gemini").is_none());
}

#[test]
fn discover_rejects_unknown_agent_ids() {
    let error = discover_agent("gemini").unwrap_err();
    assert!(format!("{error:#}").contains("unknown agent id"));
}

#[tokio::test]
async fn upgrade_rejects_unknown_agent_ids() {
    let error = upgrade_agent("gemini").await.unwrap_err();
    assert!(format!("{error:#}").contains("unknown agent id"));
}

#[test]
fn discover_never_reaches_for_a_newest_version() {
    for id in AGENT_VERSION_IDS {
        let info = discover_agent(id).unwrap();
        assert_eq!(info.latest, None, "{id}");
        assert!(!info.update, "{id}");
    }
}

/// An npm updater over a folder that exists, for tests that only need the
/// update decision.
fn npm_updater_for_test() -> Updater {
    Updater {
        via: "npm",
        program: PathBuf::from("npm"),
        args: vec![
            "install".to_owned(),
            "-g".to_owned(),
            "@openai/codex@latest".to_owned(),
        ],
        latest: LatestSource::Npm("@openai/codex".to_owned()),
        blocked: None,
    }
}
