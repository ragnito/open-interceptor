//! Convert an OpenAI Chat Completions response into an Anthropic
//! Messages response. Streaming conversion lives in a separate module
//! (T3.5) because it needs a small state machine to reassemble tool
//! call argument deltas; this file covers the simpler non-streaming
//! case.

use serde_json::Value;

use crate::translate::types_anthropic as a;
use crate::translate::types_openai as o;

/// Translate a single, fully-formed OpenAI response into Anthropic shape.
pub fn convert_non_streaming(resp: &o::ChatCompletionResponse) -> a::MessagesResponse {
    let choice = resp.choices.first();
    let assistant = choice.map(|c| &c.message);
    let finish = choice.and_then(|c| c.finish_reason);

    let mut content: Vec<a::ContentBlock> = Vec::new();

    if let Some(msg) = assistant {
        if let Some(text) = &msg.content {
            if !text.is_empty() {
                content.push(a::ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                });
            }
        }
        for tc in &msg.tool_calls {
            let input = parse_function_arguments(&tc.function.arguments);
            content.push(a::ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                input,
            });
        }
    }

    let usage = resp
        .usage
        .as_ref()
        .map(|u| a::Usage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        })
        .unwrap_or_default();

    a::MessagesResponse {
        id: resp.id.clone(),
        response_type: "message".into(),
        role: a::Role::Assistant,
        content,
        model: resp.model.clone(),
        stop_reason: finish.map(convert_finish_reason),
        stop_sequence: None,
        usage,
    }
}

/// OpenAI's `finish_reason` → Anthropic's `stop_reason`. Anthropic does
/// not have an exact equivalent for `content_filter`, so it folds into
/// `end_turn` (the model stopped, just not on its own initiative).
pub fn convert_finish_reason(reason: o::FinishReason) -> a::StopReason {
    use o::FinishReason::*;
    match reason {
        Stop => a::StopReason::EndTurn,
        Length => a::StopReason::MaxTokens,
        ToolCalls | FunctionCall => a::StopReason::ToolUse,
        ContentFilter => a::StopReason::EndTurn,
        Other => a::StopReason::EndTurn,
    }
}

/// OpenAI sends function call arguments as a stringified JSON blob. Try
/// to parse it; if it isn't valid JSON, fall back to an empty object so
/// the Anthropic shape stays well-formed (Anthropic's `input` is always
/// an object).
fn parse_function_arguments(args: &str) -> Value {
    if args.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(args).unwrap_or_else(|_| Value::Object(Default::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openai_resp(content: Option<&str>, tool_calls: Vec<o::ToolCall>, finish: o::FinishReason) -> o::ChatCompletionResponse {
        o::ChatCompletionResponse {
            id: "chatcmpl-test".into(),
            object: "chat.completion".into(),
            created: 0,
            model: "test-model".into(),
            choices: vec![o::Choice {
                index: 0,
                message: o::AssistantMessage {
                    role: Some("assistant".into()),
                    content: content.map(String::from),
                    tool_calls,
                },
                finish_reason: Some(finish),
            }],
            usage: Some(o::UsageStats {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        }
    }

    #[test]
    fn plain_text_response() {
        let resp = openai_resp(Some("hello back"), vec![], o::FinishReason::Stop);
        let out = convert_non_streaming(&resp);
        assert_eq!(out.id, "chatcmpl-test");
        assert_eq!(out.stop_reason, Some(a::StopReason::EndTurn));
        assert_eq!(out.content.len(), 1);
        match &out.content[0] {
            a::ContentBlock::Text { text, .. } => assert_eq!(text, "hello back"),
            other => panic!("expected Text, got {other:?}"),
        }
        assert_eq!(out.usage.input_tokens, 10);
        assert_eq!(out.usage.output_tokens, 5);
    }

    #[test]
    fn tool_call_response_becomes_tool_use_block() {
        let resp = openai_resp(
            None,
            vec![o::ToolCall {
                id: "call_abc".into(),
                call_type: "function".into(),
                function: o::FunctionCall {
                    name: "Read".into(),
                    arguments: r#"{"path":"/tmp/a"}"#.into(),
                },
            }],
            o::FinishReason::ToolCalls,
        );
        let out = convert_non_streaming(&resp);
        assert_eq!(out.stop_reason, Some(a::StopReason::ToolUse));
        assert_eq!(out.content.len(), 1);
        match &out.content[0] {
            a::ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "/tmp/a");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn text_plus_tool_call_response() {
        let resp = openai_resp(
            Some("let me check"),
            vec![o::ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: o::FunctionCall {
                    name: "Read".into(),
                    arguments: r#"{"path":"/a"}"#.into(),
                },
            }],
            o::FinishReason::ToolCalls,
        );
        let out = convert_non_streaming(&resp);
        assert_eq!(out.content.len(), 2);
        assert!(matches!(out.content[0], a::ContentBlock::Text { .. }));
        assert!(matches!(out.content[1], a::ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn finish_reason_length_maps_to_max_tokens() {
        let resp = openai_resp(Some("..."), vec![], o::FinishReason::Length);
        let out = convert_non_streaming(&resp);
        assert_eq!(out.stop_reason, Some(a::StopReason::MaxTokens));
    }

    #[test]
    fn malformed_arguments_become_empty_object() {
        let resp = openai_resp(
            None,
            vec![o::ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: o::FunctionCall {
                    name: "X".into(),
                    arguments: "this is not json {".into(),
                },
            }],
            o::FinishReason::ToolCalls,
        );
        let out = convert_non_streaming(&resp);
        match &out.content[0] {
            a::ContentBlock::ToolUse { input, .. } => {
                assert!(input.is_object());
                assert_eq!(input.as_object().unwrap().len(), 0);
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn empty_content_string_is_omitted() {
        // Some providers emit "" for content alongside tool_calls. We
        // shouldn't produce an empty Text block.
        let resp = openai_resp(
            Some(""),
            vec![o::ToolCall {
                id: "c".into(),
                call_type: "function".into(),
                function: o::FunctionCall {
                    name: "X".into(),
                    arguments: "{}".into(),
                },
            }],
            o::FinishReason::ToolCalls,
        );
        let out = convert_non_streaming(&resp);
        assert_eq!(out.content.len(), 1);
        assert!(matches!(out.content[0], a::ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn missing_usage_yields_zeros() {
        let mut resp = openai_resp(Some("ok"), vec![], o::FinishReason::Stop);
        resp.usage = None;
        let out = convert_non_streaming(&resp);
        assert_eq!(out.usage.input_tokens, 0);
        assert_eq!(out.usage.output_tokens, 0);
    }

    #[test]
    fn id_and_model_are_preserved() {
        let mut resp = openai_resp(Some("hi"), vec![], o::FinishReason::Stop);
        resp.id = "custom-id-123".into();
        resp.model = "kimi-k2.6".into();
        let out = convert_non_streaming(&resp);
        assert_eq!(out.id, "custom-id-123");
        assert_eq!(out.model, "kimi-k2.6");
    }
}
