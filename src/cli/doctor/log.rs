use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::{DoctorCheck, DoctorStatus};

pub(super) fn check_gateway_log() -> DoctorCheck {
    let path = super::super::runtime::default_log_file_path();
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 0 => {
            let (error_count, last_error) = scan_recent_log_errors(&path);
            if error_count > 0 {
                let mut check = DoctorCheck::new(
                    "gateway_log",
                    "Runtime log",
                    DoctorStatus::Warning,
                    format!(
                        "log available, {} bytes; this gateway run recorded {error_count} error line(s)",
                        metadata.len()
                    ),
                )
                .hint("run codex-mixin logs -n 200 for full context");
                if let Some(last_error) = last_error {
                    check = check.detail(format!("latest error: {last_error}"));
                }
                check
            } else {
                DoctorCheck::new(
                    "gateway_log",
                    "Runtime log",
                    DoctorStatus::Ok,
                    format!("log available, {} bytes", metadata.len()),
                )
                .detail(path.display().to_string())
            }
        }
        Ok(_) => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Warning,
            "log file is empty",
        )
        .detail(path.display().to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Warning,
            "log file has not been created yet",
        )
        .detail(path.display().to_string()),
        Err(error) => DoctorCheck::new(
            "gateway_log",
            "Runtime log",
            DoctorStatus::Error,
            "log file could not be read",
        )
        .detail(format!("{}: {error}", path.display())),
    }
}

const LOG_SCAN_TAIL_BYTES: u64 = 64 * 1024;
pub(super) const GATEWAY_START_MARKER: &str = "gateway process starting";

/// Counts ERROR lines that belong to the current gateway run (after the most
/// recent startup marker within the tail of the log).
fn scan_recent_log_errors(path: &Path) -> (usize, Option<String>) {
    let Ok(mut file) = fs::File::open(path) else {
        return (0, None);
    };
    let Ok(metadata) = file.metadata() else {
        return (0, None);
    };
    let offset = metadata.len().saturating_sub(LOG_SCAN_TAIL_BYTES);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (0, None);
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return (0, None);
    }
    let tail = String::from_utf8_lossy(&tail);
    let current_run = tail
        .rfind(GATEWAY_START_MARKER)
        .map_or(tail.as_ref(), |index| &tail[index..]);
    count_error_lines(current_run)
}

pub(super) fn count_error_lines(text: &str) -> (usize, Option<String>) {
    let mut count = 0;
    let mut last = None;
    for line in text.lines() {
        if line.contains(" ERROR ") {
            count += 1;
            last = Some(truncated(line, 240));
        }
    }
    (count, last)
}

fn truncated(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        return raw.trim().to_owned();
    }
    let mut text: String = raw.trim().chars().take(max_chars).collect();
    text.push_str("...");
    text
}
