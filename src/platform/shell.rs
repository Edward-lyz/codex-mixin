//! Command-line conventions of the user's shell: argument quoting, how to run
//! a Python script, and how paths are written inside instructions.

use std::path::Path;

/// Command that runs a Python 3 interpreter. Windows installs rarely expose
/// `python3`; the `py` launcher is the documented cross-version entry point.
pub const PYTHON_LAUNCHER: &str = if cfg!(windows) { "py -3" } else { "python3" };
const ENCODED_COMMAND_PREFIX: &str =
    "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ";
const REPORT_HOOK_MARKER: &str = " report-hook --event ";

/// Quote one argument for PowerShell on Windows or POSIX sh elsewhere.
pub fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        powershell_quote(value)
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(super) fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Encode a PowerShell command so cmd cannot expand path characters first.
pub(super) fn encoded_command(script: &str) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!("{ENCODED_COMMAND_PREFIX}{encoded}")
}

/// Codex hook command, whose Windows host may be cmd or PowerShell.
pub fn report_hook_command(executable: &Path, event: &str) -> String {
    if cfg!(windows) {
        encoded_command(&format!(
            "& {} report-hook --event {}; exit $LASTEXITCODE",
            powershell_quote(&executable.to_string_lossy()),
            powershell_quote(event)
        ))
    } else {
        format!(
            "{} report-hook --event {}",
            shell_quote(&executable.to_string_lossy()),
            shell_quote(event)
        )
    }
}

/// Recognize both plain and encoded hooks when updating or uninstalling them.
pub fn is_report_hook_command(command: &str) -> bool {
    use base64::Engine;
    if command.contains(REPORT_HOOK_MARKER) {
        return true;
    }
    let Some(encoded) = command.strip_prefix(ENCODED_COMMAND_PREFIX) else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return false;
    }
    let units = pairs
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    String::from_utf16(&units).is_ok_and(|script| script.contains(REPORT_HOOK_MARKER))
}

/// Path text for instructions and shell snippets. Windows paths use forward
/// slashes, which both cmd and PowerShell accept and Markdown keeps intact.
pub fn portable_path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}
