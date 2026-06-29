// Provider implementations.
//
// `anthropic` handles every upstream that speaks the Anthropic Messages
// API natively (Anthropic, DeepSeek's /anthropic, OpenRouter's Anthropic
// endpoint, etc.). `openai` and `passthrough` arrive in later phases.

pub mod anthropic;
pub mod openai;

/// Truncate a byte buffer to at most `max_bytes` for safe inclusion in a
/// log line.  Returns a UTF-8 lossy representation — bodies that are
/// already JSON decode cleanly; binary blobs surface as `<bytes>`.
///
/// Used to keep debug-level request/response dumps from filling the disk
/// when conversations carry large prompts.
pub fn truncate_for_log(body: &[u8], max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return String::from_utf8_lossy(body).into_owned();
    }
    let mut s = String::from_utf8_lossy(&body[..max_bytes]).into_owned();
    s.push_str(&format!("…(+{} bytes)", body.len() - max_bytes));
    s
}
