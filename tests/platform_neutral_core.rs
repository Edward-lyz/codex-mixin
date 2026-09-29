//! The CLI core is platform neutral: every OS difference lives behind
//! `src/platform`. This test scans production Rust code everywhere else
//! (library, CLI, TUI) and fails on platform conditionals or OS-specific
//! tools, paths, and environment variables. Test-only code (`#[cfg(test)]`
//! items and `tests.rs` files) may still gate fixtures by platform.

use std::fs;
use std::path::{Path, PathBuf};

const SCANNED_ROOTS: [&str; 2] = ["src", "tui"];
const PLATFORM_ROOT: &str = "src/platform";

/// Case-sensitive fragments that only appear in platform-specific code.
const FORBIDDEN: &[(&str, &str)] = &[
    ("cfg(unix", "platform conditional"),
    ("cfg(windows", "platform conditional"),
    ("cfg(not(unix", "platform conditional"),
    ("cfg(not(windows", "platform conditional"),
    ("target_os", "platform conditional"),
    ("target_family", "platform conditional"),
    ("cfg!(unix", "platform conditional"),
    ("cfg!(windows", "platform conditional"),
    ("any(unix", "platform conditional"),
    ("any(windows", "platform conditional"),
    ("std::os::unix", "OS-specific std extension"),
    ("std::os::windows", "OS-specific std extension"),
    ("consts::OS", "OS name branch"),
    ("consts::EXE_SUFFIX", "executable suffix"),
    (".exe\"", "Windows executable name"),
    ("USERPROFILE", "Windows home variable"),
    ("LOCALAPPDATA", "Windows directory variable"),
    ("\"APPDATA", "Windows directory variable"),
    ("var_os(\"HOME\")", "Unix home variable"),
    ("var(\"HOME\")", "Unix home variable"),
    ("env(\"HOME\"", "Unix home variable"),
    ("launchctl", "macOS service manager"),
    ("osascript", "macOS scripting tool"),
    ("codesign", "macOS signing tool"),
    ("plutil", "macOS property list tool"),
    ("/Applications/", "macOS application path"),
    ("Library/", "macOS library path"),
    ("systemctl", "Linux service manager"),
    ("schtasks", "Windows task scheduler"),
    ("taskkill", "Windows process tool"),
    ("tasklist", "Windows process tool"),
    ("powershell", "Windows shell"),
    ("cmd.exe", "Windows shell"),
    ("icacls", "Windows ACL tool"),
    ("\"/usr/", "Unix system path"),
    ("\"/bin/", "Unix system path"),
    ("process_group(0", "Unix process groups"),
    ("kill_process_group", "Unix process groups"),
    ("PermissionsExt", "Unix permission bits"),
    ("from_mode(", "Unix permission bits"),
];

#[test]
fn production_code_outside_the_platform_module_is_platform_neutral() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for scanned in SCANNED_ROOTS {
        for file in rust_files(&root.join(scanned)) {
            let relative = file
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative.starts_with(PLATFORM_ROOT) || is_test_file(&relative) {
                continue;
            }
            let source = fs::read_to_string(&file).unwrap();
            for (line_number, line) in production_lines(&source) {
                for (fragment, reason) in FORBIDDEN {
                    if line.contains(fragment) {
                        violations.push(format!(
                            "{relative}:{line_number}: {reason} (`{fragment}`): {}",
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "platform-specific code must live in src/platform:\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_skips_test_items_and_keeps_production_code() {
    let source = r##"
fn production() { let x = "{"; }
#[cfg(unix)]
fn gated() {}
#[cfg(test)]
mod tests {
    #[cfg(unix)]
    fn fixture() { let brace = '}'; let text = r#"}"#; }
}
#[cfg(all(test, unix))]
fn test_helper() {}
fn after() {}
"##;
    let lines = production_lines(source)
        .into_iter()
        .map(|(_, line)| line.trim().to_owned())
        .collect::<Vec<_>>();
    assert!(lines.contains(&"#[cfg(unix)]".to_owned()));
    assert!(lines.contains(&"fn after() {}".to_owned()));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("fixture") || line.contains("test_helper"))
    );
}

fn is_test_file(relative: &str) -> bool {
    relative.ends_with("/tests.rs") || relative.contains("/tests/")
}

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Lines outside test-only items and comments, with 1-based line numbers.
/// An item is test-only when it follows an attribute whose `cfg` mentions
/// `test`; the item ends at `;` or at the brace that closes its body.
fn production_lines(source: &str) -> Vec<(usize, String)> {
    let (masked, comment) = mask_comments_and_literals(source);
    let mut skip = vec![false; source.len()];
    let bytes = masked.as_bytes();
    let mut index = 0;
    while let Some(offset) = masked[index..].find("#[cfg") {
        let start = index + offset;
        let attribute_end = match matching(bytes, start + 1, b'[', b']') {
            Some(end) => end,
            None => break,
        };
        let attribute = &masked[start..=attribute_end];
        index = attribute_end + 1;
        if !attribute.contains("test") || attribute.contains("not(test") {
            continue;
        }
        let mut cursor = attribute_end + 1;
        let mut item_end = source.len() - 1;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b';' => {
                    item_end = cursor;
                    break;
                }
                b'{' => {
                    item_end = matching(bytes, cursor, b'{', b'}').unwrap_or(item_end);
                    break;
                }
                _ => cursor += 1,
            }
        }
        for flag in &mut skip[start..=item_end] {
            *flag = true;
        }
        index = item_end + 1;
    }
    let mut lines = Vec::new();
    let mut line_start = 0;
    for (number, line) in source.split('\n').enumerate() {
        let masked_line = &masked[line_start..line_start + line.len()];
        let production = (line_start..line_start + line.len())
            .zip(masked_line.bytes())
            .filter(|(position, byte)| !skip[*position] && !byte.is_ascii_whitespace())
            .count()
            > 0;
        if production {
            // Keep literal contents (the forbidden list inspects them) but
            // drop comments, which may discuss other platforms freely.
            let text = line
                .char_indices()
                .filter(|(offset, _)| !comment[line_start + offset])
                .map(|(_, character)| character)
                .collect();
            lines.push((number + 1, text));
        }
        line_start += line.len() + 1;
    }
    lines
}

fn matching(bytes: &[u8], open_index: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &byte) in bytes[open_index..].iter().enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(open_index + offset);
            }
        }
    }
    None
}

/// Replace comments and string/char literal contents with spaces so brace
/// matching only sees code, and flag comment bytes. Byte offsets are kept.
fn mask_comments_and_literals(source: &str) -> (String, Vec<bool>) {
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    let mut comment = vec![false; bytes.len()];
    let mut index = 0;
    let blank = |masked: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut masked[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                let end = source[index..]
                    .find('\n')
                    .map_or(bytes.len(), |end| index + end);
                blank(&mut masked, index, end);
                comment[index..end].fill(true);
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let end = source[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| index + 2 + end + 2);
                blank(&mut masked, index, end);
                comment[index..end].fill(true);
                index = end;
            }
            b'r' if matches!(bytes.get(index + 1), Some(b'#' | b'"'))
                && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric()) =>
            {
                let hashes = bytes[index + 1..]
                    .iter()
                    .take_while(|&&byte| byte == b'#')
                    .count();
                if bytes.get(index + 1 + hashes) != Some(&b'"') {
                    index += 1;
                    continue;
                }
                let terminator = format!("\"{}", "#".repeat(hashes));
                let body = index + 2 + hashes;
                let end = source[body..]
                    .find(&terminator)
                    .map_or(bytes.len(), |end| body + end + terminator.len());
                blank(
                    &mut masked,
                    body,
                    end.saturating_sub(terminator.len()).max(body),
                );
                index = end;
            }
            b'"' => {
                let mut cursor = index + 1;
                while cursor < bytes.len() && bytes[cursor] != b'"' {
                    cursor += if bytes[cursor] == b'\\' { 2 } else { 1 };
                }
                blank(&mut masked, index + 1, cursor.min(bytes.len()));
                index = cursor + 1;
            }
            b'\'' => {
                // Char literal ('x', '\n', '\u{..}') versus lifetime ('a).
                let rest = &bytes[index + 1..];
                let length = if rest.first() == Some(&b'\\') {
                    rest.iter()
                        .position(|&byte| byte == b'\'')
                        .map(|end| end + 1)
                } else if rest.len() >= 2 && rest[1] == b'\'' {
                    Some(2)
                } else {
                    source[index + 1..]
                        .chars()
                        .next()
                        .filter(|character| !character.is_ascii())
                        .map(|character| character.len_utf8() + 1)
                        .filter(|&end| rest.get(end - 1) == Some(&b'\''))
                };
                match length {
                    Some(length) => {
                        blank(&mut masked, index + 1, index + length);
                        index += length + 1;
                    }
                    None => index += 1,
                }
            }
            _ => index += 1,
        }
    }
    (String::from_utf8_lossy(&masked).into_owned(), comment)
}
