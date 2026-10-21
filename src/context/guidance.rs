pub const GUIDANCE: &str = "<context_management>\n\
You have four tools that manage conversation context. Original messages remain archived and can be restored.\n\
- mixin_context_compress: replace a contiguous range of older turns with a dense factual summary. <context_state> reports the current pressure; when it is at or above the stated threshold, compress the oldest unprotected turns before doing more work. Never compress the first user message, the latest user turn, or recent turns.\n\
- mixin_context_decompress: restore a summary block or the original messages behind it.\n\
- mixin_context_search: search archived summaries and originals.\n\
- mixin_context_status: inspect token pressure, protected ranges, tier counts and compressible ranges.\n\
When compressing, preserve goals, constraints, decisions and their reasons, file paths, function names, commands, error messages and open TODOs. Write dense bullets without preamble.\n\
</context_management>\n";

/// The instruction-only variant used by the evaluation: it asks the model to
/// keep the transcript dense itself, because advertising tools that this mode
/// does not provide makes real models emit tool-call syntax as plain text.
pub const GUIDANCE_WITHOUT_TOOLS: &str = "<context_management>\n\
The conversation transcript grows long. Keep every turn dense: preserve goals, constraints, decisions and their reasons, file paths, function names, commands, error messages and open TODOs, and drop repetition.\n\
</context_management>\n";

/// Renders the token pressure the model needs to decide when to compress.
///
/// Without it the guidance stays passive: a real model has no way to notice
/// that the view outgrew the window, so it never calls the status tool.
pub fn state_note(
    estimated_tokens: u64,
    window_tokens: u64,
    pressure_ratio: f64,
    oldest: Option<(&str, &str, &str)>,
    advise_compression: bool,
) -> String {
    let threshold = (pressure_ratio * 100.0).round();
    let mut note = if window_tokens == 0 {
        format!("current view: {estimated_tokens} estimated tokens; window unknown.\n")
    } else {
        let pressure = (estimated_tokens as f64 / window_tokens as f64 * 100.0).round();
        let verdict = if !advise_compression {
            String::new()
        } else if estimated_tokens as f64 >= window_tokens as f64 * pressure_ratio {
            format!(
                "pressure is at or above the {threshold}% threshold: compress before doing more work.\n"
            )
        } else {
            format!("below the {threshold}% threshold: keep working.\n")
        };
        format!(
            "current view: {estimated_tokens} estimated tokens of {window_tokens} ({pressure}%).\n{verdict}"
        )
    };
    if let (true, Some((start, end, tier))) = (advise_compression, oldest) {
        note.push_str(&format!(
            "oldest unprotected range: {start}..{end} ({tier}).\n"
        ));
    }
    note
}

pub fn with_guidance(instructions: Option<&str>, guidance: &str) -> String {
    match instructions {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{guidance}"),
        _ => guidance.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{GUIDANCE, state_note, with_guidance};

    #[test]
    fn reports_pressure_against_the_window() {
        assert_eq!(
            state_note(48_000, 128_000, 0.75, None, true),
            "current view: 48000 estimated tokens of 128000 (38%).\n\
             below the 75% threshold: keep working.\n"
        );
    }

    #[test]
    fn reports_the_view_without_a_window() {
        assert_eq!(
            state_note(900, 0, 0.75, None, true),
            "current view: 900 estimated tokens; window unknown.\n"
        );
    }

    #[test]
    fn points_at_the_oldest_unprotected_range() {
        let note = state_note(
            9_000,
            16_000,
            0.75,
            Some(("m000002", "m000013", "t1")),
            true,
        );
        assert!(note.contains("(56%)"));
        assert!(note.contains("oldest unprotected range: m000002..m000013 (t1)."));
    }

    #[test]
    fn asks_for_a_compression_once_the_threshold_is_reached() {
        let note = state_note(
            12_500,
            16_000,
            0.75,
            Some(("m000002", "m000013", "t1")),
            true,
        );
        assert!(note.contains("(78%)"));
        assert!(note.contains(
            "pressure is at or above the 75% threshold: compress before doing more work."
        ));
    }

    #[test]
    fn keeps_the_client_instructions_ahead_of_the_guidance() {
        let rendered = with_guidance(Some("be brief"), GUIDANCE);
        assert!(rendered.starts_with("be brief\n\n<context_management>"));
        assert!(rendered.ends_with("</context_management>\n"));
        assert_eq!(with_guidance(None, GUIDANCE), GUIDANCE);
    }
}
