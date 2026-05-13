//! Anthropic Messages API request/response types.
//!
//! This is the surface Claude Code talks to. We model only what the
//! translation layer actually needs to read or write — opaque fields like
//! `metadata` stay as `serde_json::Value` to round-trip without loss.
//!
//! Reference: https://docs.anthropic.com/en/api/messages

use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// REQUEST
// =============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,

    /// System prompt — either a plain string or an array of content
    /// blocks (Claude Code uses the string form).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,

    /// Opaque, preserved on the way upstream when staying inside the
    /// Anthropic ecosystem; dropped by the OpenAI translator because
    /// OpenAI has no equivalent field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,

    /// Extended thinking config (Anthropic-only). Stripped before
    /// sending to OpenAI-compatible upstreams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
}

/// The `system` field can be a plain string or an array of content
/// blocks (with cache control etc.). We accept both shapes.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Message content is either a plain string (shorthand for a single text
/// block) or a heterogeneous array of typed blocks.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<Value>,
    },

    Image {
        source: ImageSource,
    },

    ToolUse {
        id: String,
        name: String,
        input: Value,
    },

    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Extended-thinking block. Carries reasoning content + a signature
    /// when returned by Anthropic. Stripped by the OpenAI translator.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Redacted thinking — same role as `Thinking` but with content
    /// hidden by Anthropic's safety filter.
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String, // "base64" | "url"
    pub media_type: Option<String>,
    pub data: Option<String>,
    pub url: Option<String>,
}

/// A tool result's content can be plain text or an array of blocks
/// (e.g. text + image returned by a tool).
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the tool's input. Kept as `Value` because
    /// the schema itself is user-defined and arbitrary.
    pub input_schema: Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool {
        name: String,
    },
    /// `none` — tell the model not to call any tool.
    None,
}

// =============================================================================
// RESPONSE (non-streaming)
// =============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct MessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String, // "message"
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    /// Anthropic's recent additions (refusal, pause_turn). Unknown
    /// variants will fail to deserialize — that's intentional, we'd
    /// rather see it and decide than silently drop.
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

// =============================================================================
// RESPONSE (streaming SSE events)
// =============================================================================
//
// The Messages streaming API emits a sequence of typed events. We model
// each one so the OpenAI→Anthropic streaming translator can produce them
// in the correct order.

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent {
    MessageStart {
        message: MessageStartPayload,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: ContentBlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaPayload,
        usage: Usage,
    },
    MessageStop,
    #[allow(dead_code)]
    Ping,
}

#[derive(Debug, Serialize)]
pub struct MessageStartPayload {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String, // "message"
    pub role: Role,
    pub content: Vec<ContentBlock>, // empty
    pub model: String,
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockStart {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        thinking: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub enum ContentBlockDelta {
    TextDelta {
        text: String,
    },
    /// Tool call arguments arrive as a stream of partial JSON strings;
    /// the assembled JSON only becomes valid at the end.
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
}

#[derive(Debug, Serialize)]
pub struct MessageDeltaPayload {
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_text_request() {
        let json = r#"{
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "claude-opus-4-7");
        assert_eq!(req.messages.len(), 1);
        match &req.messages[0].content {
            MessageContent::Text(t) => assert_eq!(t, "hi"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn parses_request_with_tool_use_blocks() {
        let json = r#"{
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "system": "You are a helpful assistant.",
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "read /tmp/a"}
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "ok let me read"},
                    {"type": "tool_use", "id": "toolu_1", "name": "Read",
                     "input": {"path": "/tmp/a"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1",
                     "content": "file contents"}
                ]}
            ],
            "tools": [{
                "name": "Read",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            }]
        }"#;
        let req: MessagesRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req.system, Some(SystemPrompt::Text(_))));
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "Read");
    }

    #[test]
    fn parses_response_with_tool_use() {
        let json = r#"{
            "id": "msg_abc",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "I'll read it"},
                {"type": "tool_use", "id": "toolu_1", "name": "Read",
                 "input": {"path": "/tmp/a"}}
            ],
            "model": "claude-opus-4-7",
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {"input_tokens": 50, "output_tokens": 20}
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.content.len(), 2);
        match &resp.content[1] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "Read"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_thinking_blocks() {
        let json = r#"{
            "id": "msg_x", "type": "message", "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "let me reason", "signature": "abc"},
                {"type": "text", "text": "result"}
            ],
            "model": "claude-opus-4-7",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        match &resp.content[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "let me reason");
                assert_eq!(signature.as_deref(), Some("abc"));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_image_block() {
        let json = r#"{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0K"}}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        let out = serde_json::to_string(&block).unwrap();
        // round-trip preserves the shape (allowing for field ordering)
        let reparsed: ContentBlock = serde_json::from_str(&out).unwrap();
        assert!(matches!(reparsed, ContentBlock::Image { .. }));
    }
}
