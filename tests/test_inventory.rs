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
                .into_owned();
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
    for name in integration_tests {
        let source = fs::read_to_string(root.join("tests").join(&name)).unwrap();
        assert!(
            source.contains("#[test") || source.contains("#[tokio::test"),
            "tests/{name} has no test entry"
        );
    }
}
