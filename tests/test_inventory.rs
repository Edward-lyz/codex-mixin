//! Guard the hand-written test registries against missing or duplicate entries.
//!
//! Cargo discovers Rust integration tests automatically, but macOS Swift tests
//! are selected by `scripts/test_macos.sh` and Windows checks are registered in
//! a C# runner. A new file can otherwise compile without ever running in CI.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MACOS_TEST_DIR: &str = "macos/tests";
const MACOS_RUNNER: &str = "scripts/test_macos.sh";
const WINDOWS_RUNNER: &str = "windows/Tests/Program.cs";
const WINDOWS_UI_RUNNER: &str = "windows/UiTests/Program.cs";
const WINDOWS_APP: &str = "windows/CodexMixin/App.xaml.cs";
const WINDOWS_WORKFLOW: &str = ".github/workflows/windows.yml";
const WINDOWS_INTEGRATION_RUNNER: &str = "scripts/test_windows_integration.ps1";
const WINDOWS_INTEGRATION_SHARDS: &str = "tests/windows_integration_shards.txt";
const WINDOWS_INTEGRATION_SHARD_NAMES: &[&str] =
    &["integration-a", "integration-b", "integration-c"];
const CLIENT_AUTH_TESTS: &str = "tests/client_auth_tests.txt";
const IGNORED_TESTS: &[(&str, &str)] = &[
    ("src/ech.rs", "live_ech_comparison"),
    (
        "src/provider/auth/ducx.rs",
        "captures_real_data_report_token_without_uploading",
    ),
    ("tests/agy_dynamic.rs", "live_agy_account_reports"),
    ("tests/real_models.rs", "all_registered_models_can_say_hi"),
    (
        "tests/real_models.rs",
        "baidu_auxiliary_auto_review_alias_reaches_real_upstream",
    ),
    (
        "tests/real_models.rs",
        "baidu_reasoning_profiles_reach_real_upstream",
    ),
    (
        "tests/real_models.rs",
        "claude_desktop_can_stream_a_real_selected_model",
    ),
    ("tests/real_models.rs", "live_agy_mixed_tools"),
];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn files_with_extension(directory: &Path, extension: &str) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Find `tests/<name>.swift` references without depending on shell syntax.
fn swift_references(source: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut rest = source;
    while let Some(index) = rest.find("tests/") {
        let candidate = &rest[index + "tests/".len()..];
        let end = candidate
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || character == '_' || character == '.')
            })
            .unwrap_or(candidate.len());
        let name = &candidate[..end];
        if name.ends_with(".swift") {
            references.push(name.to_owned());
        }
        rest = &candidate[end..];
    }
    references
}

fn swift_inventory_errors(actual: &BTreeSet<String>, references: &[String]) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for reference in references {
        *counts.entry(reference.clone()).or_insert(0) += 1;
    }
    let mut errors = Vec::new();
    for (name, count) in &counts {
        if *count > 1 {
            errors.push(format!("{name} is registered {count} times"));
        }
        if !actual.contains(name) {
            errors.push(format!("{name} is registered but does not exist"));
        }
    }
    for name in actual {
        if !counts.contains_key(name) {
            errors.push(format!("{name} is not registered"));
        }
    }
    errors.sort();
    errors
}

fn table_rows(script: &str) -> Vec<Vec<&str>> {
    let start = script
        .find("TABLE='")
        .expect("macOS test runner must contain a TABLE block")
        + "TABLE='".len();
    let end = script[start..]
        .find("\n'")
        .expect("macOS test runner TABLE block must be terminated")
        + start;
    script[start..end]
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('|').collect())
        .collect()
}

fn special_names(script: &str) -> Vec<&str> {
    let declaration = script
        .lines()
        .find(|line| line.starts_with("SPECIAL=\""))
        .expect("macOS test runner must declare SPECIAL tests");
    declaration
        .trim_start_matches("SPECIAL=\"")
        .trim_end_matches('"')
        .split_whitespace()
        .collect()
}

fn csharp_check_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for marker in ["Check(\"", "AddCheck(\""] {
        let mut rest = source;
        while let Some(index) = rest.find(marker) {
            let before = rest[..index].chars().next_back();
            if marker == "Check(\""
                && before
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                rest = &rest[index + marker.len()..];
                continue;
            }
            let candidate = &rest[index + marker.len()..];
            let end = candidate
                .find('"')
                .expect("Windows check name must be terminated");
            names.push(candidate[..end].to_owned());
            rest = &candidate[end + 1..];
        }
    }
    names
}

fn windows_integration_shard_rows(source: &str) -> Vec<(String, String)> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields = line.split('|').collect::<Vec<_>>();
            assert_eq!(fields.len(), 2, "invalid Windows shard row: {line}");
            (fields[0].to_owned(), fields[1].to_owned())
        })
        .collect()
}

fn windows_integration_shard_errors(
    actual: &BTreeSet<String>,
    rows: &[(String, String)],
) -> Vec<String> {
    let mut counts = BTreeMap::new();
    let mut errors = Vec::new();
    for (shard, name) in rows {
        if !WINDOWS_INTEGRATION_SHARD_NAMES.contains(&shard.as_str()) {
            errors.push(format!("unknown Windows integration shard: {shard}"));
        }
        if !name.ends_with(".rs") || name.contains('/') || name.contains('\\') {
            errors.push(format!("invalid Windows integration test file: {name}"));
        }
        if !actual.contains(name) {
            errors.push(format!("{name} is assigned to a shard but does not exist"));
        }
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    for name in actual {
        match counts.get(name).copied().unwrap_or(0) {
            0 => errors.push(format!(
                "{name} is not assigned to a Windows integration shard"
            )),
            1 => {}
            count => errors.push(format!(
                "{name} is assigned to {count} Windows integration shards"
            )),
        }
    }
    for shard in WINDOWS_INTEGRATION_SHARD_NAMES {
        if !rows.iter().any(|(name, _)| name.as_str() == *shard) {
            errors.push(format!("{shard} has no tests"));
        }
    }
    errors.sort();
    errors
}

/// Page roles `App.OpenPage` accepts. `overview` is handled by an earlier
/// branch because it reopens the tray panel instead of a console window.
fn windows_page_roles(source: &str) -> BTreeSet<String> {
    let marker = "page is not (";
    let start = source
        .find(marker)
        .expect("App.OpenPage must validate its page argument")
        + marker.len();
    let end = source[start..]
        .find(')')
        .expect("App.OpenPage page list must be terminated")
        + start;
    let mut roles = source[start..end]
        .split(" or ")
        .map(|role| role.trim().trim_matches('"').to_owned())
        .collect::<BTreeSet<_>>();
    assert!(roles.insert("overview".to_owned()));
    roles
}

/// Page roles visited by the Windows native UI automation gallery.
fn windows_ui_page_roles(source: &str) -> BTreeSet<String> {
    let start = source
        .find("Pages =")
        .expect("Windows UI tests must declare a Pages table")
        + "Pages =".len();
    let end = source[start..]
        .find("];")
        .expect("Windows UI test Pages table must be terminated")
        + start;
    let mut roles = BTreeSet::new();
    for line in source[start..end].lines() {
        let line = line.trim();
        let Some(role) = line.strip_prefix("(\"") else {
            continue;
        };
        let role = role
            .split('"')
            .next()
            .expect("Windows UI page role must be quoted");
        roles.insert(role.to_owned());
    }
    roles
}

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|value| value == "rs") {
            files.push(path);
        }
    }
}

fn ignored_test_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut ignoring = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[ignore") {
            ignoring = true;
            continue;
        }
        if !ignoring || trimmed.starts_with("#[") || trimmed.is_empty() {
            continue;
        }
        ignoring = false;
        let declaration = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
        let declaration = declaration.strip_prefix("async ").unwrap_or(declaration);
        let declaration = declaration.strip_prefix("fn ");
        if let Some(declaration) = declaration {
            let name = declaration
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .map_or(declaration, |index| &declaration[..index]);
            names.push(name.to_owned());
        }
    }
    names
}

fn client_auth_tests(source: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut names = Vec::new();
    for (test_index, line) in lines.iter().enumerate() {
        let marker = line.trim();
        if marker != "#[test]" && marker != "#[tokio::test]" {
            continue;
        }

        let mut start = test_index;
        while start > 0 && lines[start - 1].trim().starts_with("#[") {
            start -= 1;
        }
        let Some(declaration_index) = lines[test_index + 1..]
            .iter()
            .position(|line| {
                let line = line.trim();
                let line = line.strip_prefix("pub ").unwrap_or(line);
                let line = line.strip_prefix("async ").unwrap_or(line);
                line.starts_with("fn ")
            })
            .map(|index| test_index + 1 + index)
        else {
            continue;
        };
        let declaration = lines[declaration_index].trim();
        let declaration = declaration.strip_prefix("pub ").unwrap_or(declaration);
        let declaration = declaration.strip_prefix("async ").unwrap_or(declaration);
        let Some(declaration) = declaration.strip_prefix("fn ") else {
            continue;
        };
        let name = declaration
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map_or(declaration, |index| &declaration[..index]);

        let end = lines
            .iter()
            .enumerate()
            .skip(test_index + 1)
            .find_map(|(index, line)| {
                let line = line.trim();
                if line != "#[test]" && line != "#[tokio::test]" {
                    return None;
                }
                let mut start = index;
                while start > test_index + 1 && lines[start - 1].trim().starts_with("#[") {
                    start -= 1;
                }
                Some(start)
            })
            .unwrap_or(lines.len());
        let test_source = lines[start..end].to_vec().join("\n");
        if test_source.contains("client-auth") {
            names.push(name.to_owned());
        }
    }
    names
}

fn client_auth_manifest() -> BTreeSet<(String, String)> {
    let source = fs::read_to_string(repository_root().join(CLIENT_AUTH_TESTS)).unwrap();
    let mut expected = BTreeSet::new();
    for line in source.lines().filter(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    }) {
        let fields = line.split('|').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            3,
            "invalid client-auth test manifest row: {line}"
        );
        assert!(
            matches!(
                fields[0],
                "lib" | "cors" | "gateway_http" | "gateway_startup"
            ),
            "unknown client-auth test scope: {}",
            fields[0]
        );
        let test_name = fields[2].rsplit("::").next().unwrap();
        assert!(
            !test_name.is_empty()
                && (fields[2] == test_name || fields[2].ends_with(&format!("::{test_name}"))),
            "invalid client-auth test name: {}",
            fields[2]
        );
        assert!(
            expected.insert((fields[1].to_owned(), test_name.to_owned())),
            "duplicate client-auth test manifest row: {line}"
        );
    }
    expected
}

#[test]
fn detects_missing_and_duplicate_swift_registrations() {
    let actual = BTreeSet::from(["A.swift".to_owned(), "B.swift".to_owned()]);
    let references = vec!["A.swift".to_owned(), "A.swift".to_owned()];
    let errors = swift_inventory_errors(&actual, &references);
    assert!(
        errors
            .iter()
            .any(|error| error == "A.swift is registered 2 times")
    );
    assert!(
        errors
            .iter()
            .any(|error| error == "B.swift is not registered")
    );
}

#[test]
fn detects_an_unvisited_windows_page() {
    let app = r#"if (page is not ("providers" or "agents")) throw new ArgumentException();"#;
    let runner = r#"Pages =
    [
        ("providers", "模型与服务", "models"),
    ];"#;
    assert_eq!(
        windows_page_roles(app),
        BTreeSet::from([
            "agents".to_owned(),
            "overview".to_owned(),
            "providers".to_owned()
        ])
    );
    assert_eq!(
        windows_ui_page_roles(runner),
        BTreeSet::from(["providers".to_owned()])
    );
}

#[test]
fn detects_missing_and_duplicate_windows_integration_shards() {
    let actual = BTreeSet::from(["a.rs".to_owned(), "b.rs".to_owned()]);
    let rows = vec![
        ("integration-a".to_owned(), "a.rs".to_owned()),
        ("integration-a".to_owned(), "a.rs".to_owned()),
    ];
    let errors = windows_integration_shard_errors(&actual, &rows);
    assert!(
        errors
            .iter()
            .any(|error| error == "a.rs is assigned to 2 Windows integration shards")
    );
    assert!(
        errors
            .iter()
            .any(|error| error == "b.rs is not assigned to a Windows integration shard")
    );
}

#[test]
fn ignored_rust_tests_are_allowlisted() {
    let root = repository_root();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    let actual = files
        .into_iter()
        .flat_map(|path| {
            let source = fs::read_to_string(&path).unwrap();
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            ignored_test_names(&source)
                .into_iter()
                .map(move |name| format!("{relative}:{name}"))
        })
        .collect::<BTreeSet<_>>();
    let expected = IGNORED_TESTS
        .iter()
        .map(|(path, name)| format!("{path}:{name}"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "ignored tests changed; update the explicit external-test allowlist"
    );
}

#[test]
fn client_auth_tests_are_explicitly_inventoried() {
    let root = repository_root();
    let mut test_files = Vec::new();
    rust_files(&root.join("src"), &mut test_files);
    rust_files(&root.join("tests"), &mut test_files);
    rust_files(&root.join("panel"), &mut test_files);

    let actual = test_files
        .into_iter()
        .flat_map(|path| {
            let source = fs::read_to_string(&path).unwrap();
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "tests/test_inventory.rs" {
                return Vec::new();
            }
            client_auth_tests(&source)
                .into_iter()
                .map(move |name| (relative.clone(), name))
                .collect::<Vec<_>>()
        })
        .collect::<BTreeSet<_>>();
    let expected = client_auth_manifest();
    assert_eq!(
        actual, expected,
        "client-auth test coverage changed; update tests/client_auth_tests.txt"
    );
}

#[test]
fn every_test_entry_is_registered_once() {
    let root = repository_root();
    let swift_tests = files_with_extension(&root.join(MACOS_TEST_DIR), "swift");
    let runner = fs::read_to_string(root.join(MACOS_RUNNER)).unwrap();
    let references = swift_references(&runner);
    let errors = swift_inventory_errors(&swift_tests, &references);
    assert!(errors.is_empty(), "{}", errors.join("\n"));

    let rows = table_rows(&runner);
    let mut table_names = BTreeSet::new();
    let mut row_errors = Vec::new();
    for row in &rows {
        if row.len() != 5 {
            row_errors.push(format!("malformed table row: {}", row.join("|")));
            continue;
        }
        if !table_names.insert(row[0]) {
            row_errors.push(format!("duplicate table test name: {}", row[0]));
        }
        let sources = swift_references(row[4]);
        if sources.len() != 1 {
            row_errors.push(format!(
                "{} must reference exactly one Swift test file, found {}",
                row[0],
                sources.len()
            ));
        }
    }
    let specials = special_names(&runner);
    let mut special_set = BTreeSet::new();
    for name in &specials {
        if !special_set.insert(*name) {
            row_errors.push(format!("duplicate special test name: {name}"));
        }
        if table_names.contains(name) {
            row_errors.push(format!("{name} is both a table and a special test"));
        }
        if !runner.contains(&format!("t_{name}() {{")) {
            row_errors.push(format!("special test {name} has no t_{name} function"));
        }
    }
    assert!(row_errors.is_empty(), "{}", row_errors.join("\n"));

    let windows_runner = fs::read_to_string(root.join(WINDOWS_RUNNER)).unwrap();
    let mut windows_names = BTreeSet::new();
    let mut windows_errors = Vec::new();
    for name in csharp_check_names(&windows_runner) {
        if !windows_names.insert(name.clone()) {
            windows_errors.push(format!("duplicate Windows check: {name}"));
        }
    }
    assert!(
        windows_runner.contains("--cli-only"),
        "Windows runner must separate fixture-only and real-CLI-only runs"
    );
    assert!(windows_errors.is_empty(), "{}", windows_errors.join("\n"));

    let integration_tests = files_with_extension(&root.join("tests"), "rs");
    let integration_manifest = fs::read_to_string(root.join(WINDOWS_INTEGRATION_SHARDS)).unwrap();
    let integration_rows = windows_integration_shard_rows(&integration_manifest);
    let shard_errors = windows_integration_shard_errors(&integration_tests, &integration_rows);
    assert!(shard_errors.is_empty(), "{}", shard_errors.join("\n"));

    let integration_runner = fs::read_to_string(root.join(WINDOWS_INTEGRATION_RUNNER)).unwrap();
    assert!(
        integration_runner.contains(WINDOWS_INTEGRATION_SHARDS),
        "Windows integration runner must use the guarded shard manifest"
    );
    let windows_workflow = fs::read_to_string(root.join(WINDOWS_WORKFLOW)).unwrap();
    for shard in WINDOWS_INTEGRATION_SHARD_NAMES {
        assert!(
            windows_workflow.contains(&format!("- {shard}")),
            "Windows CI matrix is missing {shard}"
        );
    }
    assert!(
        windows_workflow.contains(WINDOWS_INTEGRATION_RUNNER),
        "Windows CI must run the manifest-driven integration shards"
    );

    let app = fs::read_to_string(root.join(WINDOWS_APP)).unwrap();
    let ui_tests = fs::read_to_string(root.join(WINDOWS_UI_RUNNER)).unwrap();
    assert_eq!(
        windows_ui_page_roles(&ui_tests),
        windows_page_roles(&app),
        "Windows UI automation must visit every page App.OpenPage accepts"
    );

    for name in integration_tests {
        let source = fs::read_to_string(root.join("tests").join(&name)).unwrap();
        assert!(
            source.contains("#[test") || source.contains("#[tokio::test"),
            "tests/{name} has no test entry"
        );
    }
}
