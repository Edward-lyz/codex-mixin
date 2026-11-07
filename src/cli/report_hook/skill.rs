//! Skill-use detection for DUCX PostToolUse reporting.
//!
//! DUCX counts a skill as used when a tool call names a path inside an
//! installed skill. These rules follow the DUCX data-report hook, so Mixin
//! sends the same `upload/skill` records that DUCX sends for its own sessions.

use std::path::{Path, PathBuf};

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

/// `skills/` and the path after it. Group 1 is the boundary before `skills/`;
/// group 2 marks the bundled `.system` skills, which DUCX does not count.
const SKILL_PATH_PATTERN: &str = r"(?i)(^|[^A-Za-z0-9_.\-])skills/(\.system/)?([A-Za-z0-9._:/\-]*)";
/// Commands that only read file metadata.
const METADATA_COMMAND_PATTERN: &str = r"(?i)(^|[^A-Za-z0-9_.\-])(wc|stat|du|file|md5sum|sha1sum|sha256sum|shasum|cksum|get-item|measure-object)(\.exe)?\s";
/// Commands that list or search directories.
const INSPECTION_COMMAND_PATTERN: &str = r"(?i)(^|[^A-Za-z0-9_.\-])(ls|dir|tree|find|fd|grep|egrep|fgrep|rg|ag|ack|get-childitem|gci|select-string|sls)([^A-Za-z0-9_.\-]|$)";
/// Commands that move, copy or delete skill files.
const WRITE_COMMAND_PATTERN: &str = r"(?i)(^|[^A-Za-z0-9_.\-])(rmdir|rm|remove-item|mv|move-item|cp|copy-item|ln|truncate|set-content|out-file|add-content|new-item)\s([^|;&]*[^A-Za-z0-9_.\-])?skills/";
/// Skill subdirectories whose files count as skill resources.
const RESOURCE_DIRECTORIES: [&str; 3] = ["scripts", "references", "assets"];

/// What a tool call touched inside a skill, ordered from least to most
/// specific so that the most specific mention of one skill wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TriggerKind {
    Dir,
    Resource,
    SkillFile,
}

pub(super) struct SkillUse {
    pub(super) name: String,
    pub(super) version: Option<String>,
    pub(super) content: Option<String>,
    pub(super) trigger_kind: &'static str,
}

/// Finds the installed skill that a PostToolUse hook body uses.
///
/// The tool input is scanned as JSON text, the way DUCX scans it, so shell
/// strings, argument arrays and structured inputs follow the same rules. A
/// call that names two different skills is attributed to neither.
pub(super) fn detect(hook_body: &[u8]) -> anyhow::Result<Option<SkillUse>> {
    let Ok(payload) = serde_json::from_slice::<Value>(hook_body) else {
        return Ok(None);
    };
    let input = payload
        .get("tool_input")
        .map(Value::to_string)
        .unwrap_or_default()
        .replace('\\', "/");
    let cwd = payload.get("cwd").and_then(Value::as_str);
    let directories: Vec<String> = [
        cwd,
        payload
            .pointer("/tool_input/workdir")
            .and_then(Value::as_str),
        payload.pointer("/tool_input/cwd").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .map(|directory| directory.replace('\\', "/"))
    .collect();
    if !std::iter::once(&input)
        .chain(&directories)
        .any(|text| text.to_ascii_lowercase().contains("skills/"))
    {
        return Ok(None);
    }

    let skill_path = Regex::new(SKILL_PATH_PATTERN).context("compile DUCX skill path pattern")?;
    let metadata_command =
        Regex::new(METADATA_COMMAND_PATTERN).context("compile DUCX metadata command pattern")?;
    let inspection_command = Regex::new(INSPECTION_COMMAND_PATTERN)
        .context("compile DUCX inspection command pattern")?;
    let write_command =
        Regex::new(WRITE_COMMAND_PATTERN).context("compile DUCX write command pattern")?;

    // A mention is the text after `skills/` plus whether its command only lists
    // or searches. Shell separators split the input so that one command does
    // not change how the paths of the next command count.
    let mut mentions = Vec::new();
    for segment in input.split(['&', ';', '|']) {
        if metadata_command.is_match(segment) || write_command.is_match(segment) {
            continue;
        }
        let inspected = inspection_command.is_match(segment);
        for captures in skill_path.captures_iter(segment) {
            let (Some(boundary), None, Some(path)) =
                (captures.get(1), captures.get(2), captures.get(3))
            else {
                continue;
            };
            // A path that output is redirected into is written, not used.
            let head = segment[..boundary.end()]
                .trim_end_matches(|character: char| !character.is_whitespace() && character != '>')
                .trim_end();
            if !head.ends_with('>') {
                mentions.push((path.as_str(), inspected));
            }
        }
    }
    // Working inside a skill directory uses that skill unless the call only
    // lists or searches it.
    let inspected = inspection_command.is_match(&input);
    for directory in &directories {
        for captures in skill_path.captures_iter(directory) {
            if let (None, Some(path)) = (captures.get(2), captures.get(3)) {
                mentions.push((path.as_str(), inspected));
            }
        }
    }

    let roots = skill_roots(cwd.map(Path::new));
    let mut used: Option<(PathBuf, TriggerKind)> = None;
    for (path, inspected) in mentions {
        let Some((directory, kind)) = resolve(&roots, path) else {
            continue;
        };
        if inspected && kind == TriggerKind::Dir {
            continue;
        }
        match &mut used {
            Some((used_directory, used_kind)) if *used_directory == directory => {
                *used_kind = (*used_kind).max(kind);
            }
            Some(_) => return Ok(None),
            None => used = Some((directory, kind)),
        }
    }
    let Some((directory, kind)) = used else {
        return Ok(None);
    };

    let meta = std::fs::read(directory.join(".skill-meta.json"))
        .ok()
        .and_then(|contents| serde_json::from_slice::<Value>(&contents).ok());
    let meta_field = |field: &str| {
        meta.as_ref()
            .and_then(|meta| meta.get(field))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let Some(name) = meta_field("full_name").or_else(|| {
        directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }) else {
        return Ok(None);
    };
    Ok(Some(SkillUse {
        name,
        version: meta_field("version"),
        content: std::fs::read_to_string(directory.join("SKILL.md"))
            .ok()
            .filter(|content| !content.is_empty()),
        trigger_kind: match kind {
            TriggerKind::Dir => "dir",
            TriggerKind::Resource => "resource",
            TriggerKind::SkillFile => "skillmd",
        },
    }))
}

/// Skill directories in the order DUCX searches them: the user's Codex and
/// agent skills, then the project's skills from the working directory up to
/// its git root.
fn skill_roots(cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![
        crate::cli::codex::codex_home_path().join("skills"),
        codex_mixin::platform::home_dir()
            .join(".agents")
            .join("skills"),
    ];
    let Some(cwd) = cwd else {
        return roots;
    };
    roots.push(cwd.join(".codex").join("skills"));
    let git_root = cwd
        .ancestors()
        .find(|directory| directory.join(".git").exists());
    for directory in cwd.ancestors() {
        roots.push(directory.join(".agents").join("skills"));
        if git_root.is_none_or(|root| root == directory) {
            break;
        }
    }
    roots
}

/// Maps the text after `skills/` to an installed skill directory and the part
/// of the skill it names. DUCX looks the path up by skill name, so the
/// directory in front of `skills/` does not matter.
fn resolve(roots: &[PathBuf], path: &str) -> Option<(PathBuf, TriggerKind)> {
    let components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    let first = *components.first()?;
    if first.eq_ignore_ascii_case(".system") || components.contains(&"..") {
        return None;
    }
    let root = roots.iter().find(|root| root.join(first).is_dir())?;
    // The skill is the first directory on the path that has a SKILL.md. A
    // top-level directory without one still counts as a skill.
    let (mut skill, mut depth) = (root.join(first), 1);
    let mut directory = root.clone();
    for (index, component) in components.iter().enumerate() {
        directory.push(component);
        if !directory.is_dir() {
            break;
        }
        if directory.join("SKILL.md").is_file() {
            (skill, depth) = (directory, index + 1);
            break;
        }
    }
    let kind = match &components[depth..] {
        [file, ..] if file.eq_ignore_ascii_case("SKILL.md") => TriggerKind::SkillFile,
        [directory, _, ..] if RESOURCE_DIRECTORIES.contains(directory) => TriggerKind::Resource,
        _ => TriggerKind::Dir,
    };
    Some((skill, kind))
}
