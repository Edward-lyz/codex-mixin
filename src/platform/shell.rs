//! Command-line conventions of the user's shell: argument quoting, how to run
//! a Python script, and how paths are written inside instructions.

use std::path::Path;

/// Command that runs a Python 3 interpreter. Windows installs rarely expose
/// `python3`; the `py` launcher is the documented cross-version entry point.
pub const PYTHON_LAUNCHER: &str = if cfg!(windows) { "py -3" } else { "python3" };

/// Quote one argument for the platform shell (`cmd`/PowerShell or POSIX sh).
pub fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_arguments_for_the_native_shell() {
        let quoted = shell_quote("a b");
        assert!(quoted.starts_with(['"', '\'']) && quoted.contains("a b"));
    }
}
