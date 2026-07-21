use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::{Value, json};
use walkdir::WalkDir;

const MAX_RESULT_BYTES: usize = 32 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
const MAX_GREP_MATCHES: usize = 200;

#[derive(Clone, Debug)]
pub struct PanelToolExecutor {
    root: PathBuf,
}

impl PanelToolExecutor {
    pub fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| anyhow::anyhow!("canonicalize panel tool root: {error}"))?;
        if !root.is_dir() {
            anyhow::bail!("panel tool root is not a directory: {}", root.display());
        }
        Ok(Self { root })
    }

    pub fn schemas() -> Vec<Value> {
        vec![
            function_schema(
                "read_file",
                "Read a file inside the workspace. Results are truncated to 32 KiB.",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "offset":{"type":"integer","minimum":0},
                        "limit":{"type":"integer","minimum":1}
                    },
                    "required":["path"],
                    "additionalProperties":false
                }),
            ),
            function_schema(
                "list_files",
                "List files under a workspace directory, optionally filtered by a glob.",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "glob":{"type":"string"}
                    },
                    "additionalProperties":false
                }),
            ),
            function_schema(
                "grep",
                "Search workspace files using a regular expression.",
                json!({
                    "type":"object",
                    "properties":{
                        "pattern":{"type":"string"},
                        "path":{"type":"string"},
                        "glob":{"type":"string"}
                    },
                    "required":["pattern"],
                    "additionalProperties":false
                }),
            ),
            function_schema(
                "git_inspect",
                "Run a read-only git status, log, diff, or show command in the workspace.",
                json!({
                    "type":"object",
                    "properties":{
                        "subcommand":{"type":"string","enum":["status","log","diff","show"]},
                        "args":{"type":"array","items":{"type":"string"},"maxItems":32}
                    },
                    "required":["subcommand"],
                    "additionalProperties":false
                }),
            ),
        ]
    }

    pub fn execute(&self, name: &str, arguments: &str) -> Result<String, String> {
        let arguments: Value = serde_json::from_str(arguments)
            .map_err(|error| format!("invalid {name} arguments: {error}"))?;
        match name {
            "read_file" => self.read_file(&arguments),
            "list_files" => self.list_files(&arguments),
            "grep" => self.grep(&arguments),
            "git_inspect" => self.git_inspect(&arguments),
            _ => Err(format!("unsupported panel tool: {name}")),
        }
    }

    fn read_file(&self, arguments: &Value) -> Result<String, String> {
        let path = required_string(arguments, "path")?;
        let path = self.resolve_existing(path)?;
        if !path.is_file() {
            return Err(format!("not a file: {}", path.display()));
        }
        let offset = optional_usize(arguments, "offset")?.unwrap_or(0);
        let requested = optional_usize(arguments, "limit")?.unwrap_or(MAX_RESULT_BYTES);
        let mut file =
            fs::File::open(&path).map_err(|error| format!("open {}: {error}", path.display()))?;
        let file_len = file
            .metadata()
            .map_err(|error| format!("stat {}: {error}", path.display()))?
            .len();
        if u64::try_from(offset).unwrap_or(u64::MAX) >= file_len {
            return Ok(String::new());
        }
        file.seek(SeekFrom::Start(u64::try_from(offset).unwrap_or(u64::MAX)))
            .map_err(|error| format!("seek {}: {error}", path.display()))?;
        let read_limit = requested.min(MAX_RESULT_BYTES);
        let mut bytes = vec![0; read_limit];
        let read = file
            .read(&mut bytes)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        bytes.truncate(read);
        let mut result = String::from_utf8_lossy(&bytes).into_owned();
        if u64::try_from(offset.saturating_add(read)).unwrap_or(u64::MAX) < file_len {
            result.push_str("\n[truncated]");
        }
        Ok(result)
    }

    fn list_files(&self, arguments: &Value) -> Result<String, String> {
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let path = self.resolve_existing(path)?;
        let matcher = optional_glob(arguments)?;
        let mut files = Vec::new();
        for entry in WalkDir::new(&path).follow_links(false) {
            let entry = entry.map_err(|error| format!("walk {}: {error}", path.display()))?;
            if entry.file_type().is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let canonical = entry
                .path()
                .canonicalize()
                .map_err(|error| format!("canonicalize {}: {error}", entry.path().display()))?;
            self.ensure_inside(&canonical)?;
            let relative = canonical
                .strip_prefix(&self.root)
                .expect("validated workspace path")
                .to_string_lossy()
                .into_owned();
            if matcher
                .as_ref()
                .is_none_or(|matcher| glob_matches(matcher, &relative))
            {
                files.push(relative);
                if files.len() == MAX_LIST_ENTRIES {
                    files.push("[truncated]".to_owned());
                    break;
                }
            }
        }
        Ok(files.join("\n"))
    }

    fn grep(&self, arguments: &Value) -> Result<String, String> {
        let pattern = required_string(arguments, "pattern")?;
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let path = self.resolve_existing(path)?;
        let glob = arguments.get("glob").and_then(Value::as_str);
        match self.grep_with_rg(pattern, &path, glob) {
            Ok(result) => Ok(result),
            Err(RgError::Unavailable) => self.grep_in_process(pattern, &path, glob),
            Err(RgError::Failed(message)) => Err(message),
        }
    }

    fn grep_with_rg(
        &self,
        pattern: &str,
        path: &Path,
        glob: Option<&str>,
    ) -> Result<String, RgError> {
        let mut command = Command::new("rg");
        command.current_dir(&self.root).args([
            "--no-heading",
            "--line-number",
            "--color",
            "never",
            "--max-count",
            &MAX_GREP_MATCHES.to_string(),
        ]);
        if let Some(glob) = glob {
            command.arg("-g").arg(glob);
        }
        command.arg("--").arg(pattern).arg(path);
        let output = command.output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RgError::Unavailable
            } else {
                RgError::Failed(format!("run rg: {error}"))
            }
        })?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(RgError::Failed(format!(
                "rg failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(truncate_bytes(&output.stdout))
    }

    fn grep_in_process(
        &self,
        pattern: &str,
        path: &Path,
        glob: Option<&str>,
    ) -> Result<String, String> {
        let regex = Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?;
        let matcher = glob.map(glob_regex).transpose()?;
        let mut matches = Vec::new();
        for entry in WalkDir::new(path).follow_links(false) {
            let entry = entry.map_err(|error| format!("walk {}: {error}", path.display()))?;
            if entry.file_type().is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let canonical = entry
                .path()
                .canonicalize()
                .map_err(|error| format!("canonicalize {}: {error}", entry.path().display()))?;
            self.ensure_inside(&canonical)?;
            let relative = canonical
                .strip_prefix(&self.root)
                .expect("validated workspace path")
                .to_string_lossy();
            if matcher
                .as_ref()
                .is_some_and(|matcher| !glob_matches(matcher, &relative))
            {
                continue;
            }
            let Ok(content) = fs::read_to_string(&canonical) else {
                continue;
            };
            for (line_index, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    matches.push(format!("{relative}:{}:{line}", line_index + 1));
                    if matches.len() == MAX_GREP_MATCHES {
                        matches.push("[truncated]".to_owned());
                        return Ok(truncate_string(matches.join("\n")));
                    }
                }
            }
        }
        Ok(truncate_string(matches.join("\n")))
    }

    fn git_inspect(&self, arguments: &Value) -> Result<String, String> {
        let subcommand = required_string(arguments, "subcommand")?;
        if !matches!(subcommand, "status" | "log" | "diff" | "show") {
            return Err(format!("git subcommand is not allowed: {subcommand}"));
        }
        let args = match arguments.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(args)) => args
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "git args must contain only strings".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("git args must be an array".to_owned()),
        };
        for arg in &args {
            validate_git_arg(arg)?;
        }
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env_remove("GIT_EXTERNAL_DIFF")
            .arg("--no-pager")
            .arg(subcommand)
            .args(&args)
            .output()
            .map_err(|error| format!("run git {subcommand}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {subcommand} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(truncate_bytes(&output.stdout))
    }

    fn resolve_existing(&self, path: &str) -> Result<PathBuf, String> {
        if path.contains('\0') {
            return Err("path contains a NUL byte".to_owned());
        }
        let path = Path::new(path);
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let canonical = joined
            .canonicalize()
            .map_err(|error| format!("canonicalize {}: {error}", joined.display()))?;
        self.ensure_inside(&canonical)?;
        Ok(canonical)
    }

    fn ensure_inside(&self, path: &Path) -> Result<(), String> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(format!(
                "path escapes panel tool workspace: {}",
                path.display()
            ))
        }
    }
}

enum RgError {
    Unavailable,
    Failed(String),
}

fn function_schema(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type":"function",
        "name":name,
        "description":description,
        "parameters":parameters,
        "strict":true
    })
}

fn required_string<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} must be a non-empty string"))
}

fn optional_usize(arguments: &Value, name: &str) -> Result<Option<usize>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("{name} must be a non-negative integer"))
        })
        .transpose()
}

fn optional_glob(arguments: &Value) -> Result<Option<Regex>, String> {
    arguments
        .get("glob")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "glob must be a string".to_owned())
                .and_then(glob_regex)
        })
        .transpose()
}

fn glob_regex(pattern: &str) -> Result<Regex, String> {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                regex.push_str(".*");
            }
            '*' => regex.push_str("[^/]*"),
            '?' => regex.push_str("[^/]"),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|error| format!("invalid glob: {error}"))
}

fn glob_matches(regex: &Regex, relative: &str) -> bool {
    regex.is_match(relative)
        || (!relative.contains('/') && regex.is_match(relative))
        || relative
            .rsplit_once('/')
            .is_some_and(|(_, file_name)| regex.is_match(file_name))
}

fn validate_git_arg(arg: &str) -> Result<(), String> {
    const FORBIDDEN: &[&str] = &[
        "-o",
        "--output",
        "--ext-diff",
        "--textconv",
        "--no-index",
        "--git-dir",
        "--work-tree",
        "--config-env",
        "--pathspec-from-file",
        "--exec",
    ];
    if arg.contains(['\0', '\n', '\r'])
        || FORBIDDEN
            .iter()
            .any(|forbidden| arg == *forbidden || arg.starts_with(&format!("{forbidden}=")))
        || Path::new(arg).is_absolute()
        || Path::new(arg)
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(format!("git argument is not allowed: {arg}"));
    }
    Ok(())
}

fn truncate_bytes(bytes: &[u8]) -> String {
    truncate_string(String::from_utf8_lossy(bytes).into_owned())
}

fn truncate_string(mut value: String) -> String {
    if value.len() <= MAX_RESULT_BYTES {
        return value;
    }
    let mut boundary = MAX_RESULT_BYTES;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value.push_str("\n[truncated]");
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_escape_and_unsafe_git_commands() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("workspace");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("inside.txt"), "inside").unwrap();
        fs::write(base.path().join("outside.txt"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.path().join("outside.txt"), root.join("escape-link"))
            .unwrap();
        let executor = PanelToolExecutor::new(&root).unwrap();
        assert_eq!(
            executor
                .execute("read_file", r#"{"path":"inside.txt"}"#)
                .unwrap(),
            "inside"
        );
        assert!(
            executor
                .execute("read_file", r#"{"path":"../outside.txt"}"#)
                .is_err()
        );
        #[cfg(unix)]
        assert!(
            executor
                .execute("read_file", r#"{"path":"escape-link"}"#)
                .is_err()
        );
        assert!(
            executor
                .execute("git_inspect", r#"{"subcommand":"checkout"}"#)
                .is_err()
        );
        assert!(
            executor
                .execute(
                    "git_inspect",
                    r#"{"subcommand":"diff","args":["--output=/tmp/leak"]}"#
                )
                .is_err()
        );
    }
}
