pub const GUIDANCE: &str = "<context_management>\n\
You have four tools that manage conversation context. Original messages remain archived and can be restored.\n\
- mixin_context_compress: replace a contiguous range of older turns with a dense factual summary. Compress only when mixin_context_status reports pressure or growth. Never compress the first user message, the latest user turn, or recent turns.\n\
- mixin_context_decompress: restore a summary block or the original messages behind it.\n\
- mixin_context_search: search archived summaries and originals.\n\
- mixin_context_status: inspect token pressure, protected ranges, tier counts and compressible ranges.\n\
When compressing, preserve goals, constraints, decisions and their reasons, file paths, function names, commands, error messages and open TODOs. Write dense bullets without preamble.\n\
</context_management>\n";

pub fn with_guidance(instructions: Option<&str>) -> String {
    match instructions {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{GUIDANCE}"),
        _ => GUIDANCE.to_owned(),
    }
}
