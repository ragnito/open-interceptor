//! Context-window guard: estimates input tokens and decides whether a request
//! should be rejected with a "prompt is too long" error before forwarding.
//!
//! Token estimation uses the same chars/4 heuristic already used in
//! `translate::sse_stream` for output estimation. It is intentionally
//! conservative (may overestimate) so that the guard fires slightly before the
//! real upstream limit, giving Claude Code room to compact.

use axum::body::Bytes;

use crate::translate::types_anthropic::{
    ContentBlock, MessageContent, MessagesRequest, SystemPrompt, ToolResultContent,
};

/// Estimate the number of input tokens in a Messages request body.
///
/// Parses the body as an Anthropic `MessagesRequest` and sums character
/// lengths across `system`, all `messages`, and `tools`, then divides by 4.
/// Falls back to `body.len() / 4` if the body cannot be parsed.
pub fn estimate_input_tokens(body: &Bytes) -> u32 {
    match serde_json::from_slice::<MessagesRequest>(body) {
        Ok(req) => estimate_from_request(&req),
        Err(_) => (body.len() / 4) as u32,
    }
}

/// Decide whether `estimated` tokens exceed `context_window * threshold`.
///
/// Returns `Some(limit)` — the effective token limit — when the guard should
/// fire. Returns `None` when the request is within bounds.
pub fn exceeds(estimated: u32, context_window: u32, threshold: f32) -> Option<u32> {
    let limit = (context_window as f32 * threshold).floor() as u32;
    if estimated > limit { Some(limit) } else { None }
}

// ---------------------------------------------------------------------------

fn estimate_from_request(req: &MessagesRequest) -> u32 {
    let mut chars: usize = 0;

    if let Some(system) = &req.system {
        chars += system_chars(system);
    }

    for msg in &req.messages {
        chars += message_content_chars(&msg.content);
    }

    for tool in &req.tools {
        if let Some(desc) = &tool.description {
            chars += desc.len();
        }
        chars += tool.name.len();
        // input_schema is a JSON value — use its serialized length as proxy
        chars += tool.input_schema.to_string().len();
    }

    ((chars as f64) / 4.0).ceil() as u32
}

fn system_chars(system: &SystemPrompt) -> usize {
    match system {
        SystemPrompt::Text(t) => t.len(),
        SystemPrompt::Blocks(blocks) => blocks.iter().map(|b| b.text.len()).sum(),
    }
}

fn message_content_chars(content: &MessageContent) -> usize {
    match content {
        MessageContent::Text(t) => t.len(),
        MessageContent::Blocks(blocks) => blocks.iter().map(content_block_chars).sum(),
    }
}

fn content_block_chars(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text, .. } => text.len(),
        ContentBlock::Thinking { thinking, .. } => thinking.len(),
        ContentBlock::RedactedThinking { data } => data.len(),
        ContentBlock::ToolUse { input, name, .. } => name.len() + input.to_string().len(),
        ContentBlock::ToolResult { content, .. } => tool_result_content_chars(content),
        ContentBlock::Image { .. } => 0,
    }
}

fn tool_result_content_chars(content: &ToolResultContent) -> usize {
    match content {
        ToolResultContent::Text(t) => t.len(),
        ToolResultContent::Blocks(blocks) => blocks.iter().map(content_block_chars).sum(),
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_body(messages_chars: usize) -> Bytes {
        let text = "a".repeat(messages_chars);
        let body = json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1000,
            "messages": [{"role": "user", "content": text}]
        });
        Bytes::from(serde_json::to_vec(&body).unwrap())
    }

    #[test]
    fn estimate_text_message() {
        // 400 chars → 100 tokens
        let tokens = estimate_input_tokens(&make_body(400));
        assert_eq!(tokens, 100);
    }

    #[test]
    fn estimate_fallback_on_bad_json() {
        let body = Bytes::from_static(b"not valid json");
        // 14 bytes / 4 = 3
        assert_eq!(estimate_input_tokens(&body), 3);
    }

    #[test]
    fn exceeds_returns_some_when_over_threshold() {
        // context_window=1000, threshold=0.85 → limit=850
        assert_eq!(exceeds(900, 1000, 0.85), Some(850));
    }

    #[test]
    fn exceeds_returns_none_when_within_threshold() {
        assert_eq!(exceeds(800, 1000, 0.85), None);
    }

    #[test]
    fn exceeds_exactly_at_limit_is_allowed() {
        // 850 == limit → no block
        assert_eq!(exceeds(850, 1000, 0.85), None);
    }

    #[test]
    fn exceeds_one_over_limit_is_blocked() {
        assert_eq!(exceeds(851, 1000, 0.85), Some(850));
    }
}
