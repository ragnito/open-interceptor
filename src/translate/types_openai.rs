//! OpenAI Chat Completions API request/response types.
//!
//! Only fields the translation layer reads or writes are modeled. We do
//! not pretend to cover the whole OpenAI surface — that would be wasted
//! effort and a perpetual maintenance burden.
//!
//! Reference: https://platform.openai.com/docs/api-reference/chat

use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// REQUEST
// =============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ChatTool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ChatToolChoice>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// OpenAI accepts a single string OR an array of strings here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopValue>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StopValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    System {
        content: String,
    },
    User {
        content: UserContent,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// User content can be a plain string OR an array of typed parts (text +
/// image_url) when sending images.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Parts(Vec<UserPart>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ImageUrl {
    /// Either a real URL or a `data:` URI carrying base64-encoded bytes.
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String, // "function"
    pub function: FunctionCall,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Stringified JSON; OpenAI sends the arguments as a string-encoded
    /// blob even though the contents are JSON.
    pub arguments: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatTool {
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    pub function: ChatToolFunction,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatToolFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value, // JSON Schema, opaque to us
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ChatToolChoice {
    /// "auto" | "none" | "required"
    Mode(String),
    Specific {
        #[serde(rename = "type")]
        choice_type: String, // "function"
        function: NameRef,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NameRef {
    pub name: String,
}

// =============================================================================
// RESPONSE (non-streaming)
// =============================================================================

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    #[serde(default)]
    pub object: String, // "chat.completion"
    #[serde(default)]
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<UsageStats>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: AssistantMessage,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

/// The assistant message shape inside a Choice — like `ChatMessage::Assistant`
/// but flattened (no role tag) because the parent already implies the role.
#[derive(Debug, Deserialize, Serialize)]
pub struct AssistantMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    /// Some providers (OpenCode Go among them) sometimes emit
    /// `function_call` — older OpenAI shape kept for compat.
    FunctionCall,
    ContentFilter,
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct UsageStats {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

// =============================================================================
// RESPONSE (streaming SSE chunks)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<UsageStats>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkChoice {
    #[allow(dead_code)]
    pub index: u32,
    pub delta: ChoiceDelta,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceDelta {
    #[allow(dead_code)]
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallDelta>,
}

/// In streaming mode, OpenAI sends tool calls as a sequence of deltas
/// keyed by `index`. The `id`, `name`, and `arguments` may arrive in
/// pieces across multiple chunks. The translator maintains state to
/// reassemble them.
#[derive(Debug, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_request() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "be helpful"},
                {"role": "user", "content": "hi"}
            ],
            "max_tokens": 100
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 2);
        match &req.messages[0] {
            ChatMessage::System { content } => assert_eq!(content, "be helpful"),
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn parses_assistant_with_tool_calls() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {"id":"call_1","type":"function",
                     "function":{"name":"Read","arguments":"{\"path\":\"/tmp/a\"}"}}
                ]
            }]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        match &req.messages[0] {
            ChatMessage::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].function.name, "Read");
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn parses_tool_role_message() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "file contents here"
            }]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        match &req.messages[0] {
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(content, "file contents here");
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn parses_non_streaming_response() {
        let json = r#"{
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi back"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        }"#;
        let resp: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("hi back"));
    }

    #[test]
    fn parses_streaming_chunk_with_tool_call_delta() {
        let json = r#"{
            "id": "chatcmpl-1",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [
                        {"index": 0, "id": "call_x",
                         "function": {"name": "Read", "arguments": "{\"pa"}}
                    ]
                },
                "finish_reason": null
            }]
        }"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_x"));
        let f = tc.function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("Read"));
        assert_eq!(f.arguments.as_deref(), Some("{\"pa"));
    }

    #[test]
    fn parses_user_with_image_part() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{
                "role": "user",
                "content": [
                    {"type":"text","text":"what's in this image?"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,iVBOR"}}
                ]
            }]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        match &req.messages[0] {
            ChatMessage::User {
                content: UserContent::Parts(parts),
            } => {
                assert_eq!(parts.len(), 2);
            }
            other => panic!("expected User with parts, got {other:?}"),
        }
    }
}
