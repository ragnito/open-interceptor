//! Convert an Anthropic Messages API request into an OpenAI Chat
//! Completions request. The translator is intentionally direct: every
//! transformation is named explicitly so it's obvious what changes shape
//! and what gets dropped (thinking blocks, top_k, metadata, …).

use crate::translate::types_anthropic as a;
use crate::translate::types_openai as o;

/// Translate an entire request. Borrows the input so callers can keep
/// the original around (useful when logging or for fallback paths).
pub fn convert(req: &a::MessagesRequest) -> o::ChatCompletionRequest {
    let mut messages: Vec<o::ChatMessage> = Vec::new();

    // 1. System prompt → leading role:system message (OpenAI shape).
    if let Some(system) = &req.system {
        let text = flatten_system(system);
        if !text.is_empty() {
            messages.push(o::ChatMessage::System { content: text });
        }
    }

    // 2. Each Anthropic message expands into one OR more OpenAI messages
    //    because tool_result blocks become standalone role:tool messages.
    for msg in &req.messages {
        translate_message(msg, &mut messages);
    }

    // 3. Tools: rewrap each Anthropic tool definition as an OpenAI
    //    function tool. The input_schema is JSON Schema either way.
    let tools: Vec<o::ChatTool> = req
        .tools
        .iter()
        .map(|t| o::ChatTool {
            tool_type: "function".into(),
            function: o::ChatToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect();

    // 4. Tool choice.
    let tool_choice = req.tool_choice.as_ref().map(convert_tool_choice);

    // 5. Sampling params + stop sequences.
    let stop = match req.stop_sequences.len() {
        0 => None,
        1 => Some(o::StopValue::Single(req.stop_sequences[0].clone())),
        _ => Some(o::StopValue::Many(req.stop_sequences.clone())),
    };

    // 6. Build the request. `top_k`, `metadata`, and `thinking` have no
    //    OpenAI equivalent and are intentionally dropped.
    o::ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        max_tokens: Some(req.max_tokens),
        tools,
        tool_choice,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: req.stream,
        stop,
    }
}

fn flatten_system(sp: &a::SystemPrompt) -> String {
    match sp {
        a::SystemPrompt::Text(s) => s.clone(),
        a::SystemPrompt::Blocks(blocks) => blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

fn convert_tool_choice(tc: &a::ToolChoice) -> o::ChatToolChoice {
    match tc {
        a::ToolChoice::Auto => o::ChatToolChoice::Mode("auto".into()),
        a::ToolChoice::Any => o::ChatToolChoice::Mode("required".into()),
        a::ToolChoice::None => o::ChatToolChoice::Mode("none".into()),
        a::ToolChoice::Tool { name } => o::ChatToolChoice::Specific {
            choice_type: "function".into(),
            function: o::NameRef { name: name.clone() },
        },
    }
}

/// Translate a single Anthropic message. May emit multiple OpenAI
/// messages because tool_result blocks turn into separate role:tool
/// entries that must appear at their original position in the
/// conversation.
fn translate_message(msg: &a::Message, out: &mut Vec<o::ChatMessage>) {
    let blocks: Vec<&a::ContentBlock> = match &msg.content {
        a::MessageContent::Text(t) => {
            // Shorthand: a plain string is a single text block.
            match msg.role {
                a::Role::User => out.push(o::ChatMessage::User {
                    content: o::UserContent::Text(t.clone()),
                }),
                a::Role::Assistant => out.push(o::ChatMessage::Assistant {
                    content: Some(t.clone()),
                    tool_calls: vec![],
                }),
            }
            return;
        }
        a::MessageContent::Blocks(b) => b.iter().collect(),
    };

    match msg.role {
        a::Role::User => translate_user_blocks(&blocks, out),
        a::Role::Assistant => translate_assistant_blocks(&blocks, out),
    }
}

/// User messages can mix text, images, and tool_results. Tool results
/// must become role:tool messages at their original position so the
/// upstream model sees the same conversation order.
fn translate_user_blocks(blocks: &[&a::ContentBlock], out: &mut Vec<o::ChatMessage>) {
    let mut pending_parts: Vec<o::UserPart> = Vec::new();

    let flush_pending = |pending: &mut Vec<o::UserPart>, out: &mut Vec<o::ChatMessage>| {
        if pending.is_empty() {
            return;
        }
        // Collapse to a plain string if the pending list is just one text
        // — keeps simple cases simple and matches what most APIs prefer.
        let content = if pending.len() == 1 {
            if let o::UserPart::Text { text } = &pending[0] {
                o::UserContent::Text(text.clone())
            } else {
                o::UserContent::Parts(std::mem::take(pending))
            }
        } else {
            o::UserContent::Parts(std::mem::take(pending))
        };
        out.push(o::ChatMessage::User { content });
    };

    for block in blocks {
        match block {
            a::ContentBlock::Text { text, .. } => {
                pending_parts.push(o::UserPart::Text { text: text.clone() });
            }
            a::ContentBlock::Image { source } => {
                if let Some(part) = image_to_user_part(source) {
                    pending_parts.push(part);
                }
            }
            a::ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                // Flush any pending text/image before the tool message so
                // ordering matches the original conversation.
                flush_pending(&mut pending_parts, out);
                let content_str = flatten_tool_result(content);
                out.push(o::ChatMessage::Tool {
                    tool_call_id: tool_use_id.clone(),
                    content: content_str,
                });
            }
            // Thinking blocks have no OpenAI analog and are stripped.
            a::ContentBlock::Thinking { .. } | a::ContentBlock::RedactedThinking { .. } => {}
            // tool_use shouldn't appear in user messages but if it does we drop it.
            a::ContentBlock::ToolUse { .. } => {}
        }
    }
    flush_pending(&mut pending_parts, out);
}

/// Assistant messages can have text + tool_use blocks. They collapse
/// into a single OpenAI message with optional content and a list of
/// tool_calls.
fn translate_assistant_blocks(blocks: &[&a::ContentBlock], out: &mut Vec<o::ChatMessage>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<o::ToolCall> = Vec::new();

    for block in blocks {
        match block {
            a::ContentBlock::Text { text, .. } => text_parts.push(text.clone()),
            a::ContentBlock::ToolUse { id, name, input } => {
                let arguments = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                tool_calls.push(o::ToolCall {
                    id: id.clone(),
                    call_type: "function".into(),
                    function: o::FunctionCall {
                        name: name.clone(),
                        arguments,
                    },
                });
            }
            // Thinking blocks: OpenAI has no equivalent → drop.
            a::ContentBlock::Thinking { .. } | a::ContentBlock::RedactedThinking { .. } => {}
            // tool_result / image shouldn't appear in assistant messages.
            _ => {}
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };

    out.push(o::ChatMessage::Assistant {
        content,
        tool_calls,
    });
}

fn image_to_user_part(src: &a::ImageSource) -> Option<o::UserPart> {
    match src.source_type.as_str() {
        "base64" => {
            let media_type = src.media_type.as_deref().unwrap_or("image/png");
            let data = src.data.as_deref()?;
            let url = format!("data:{media_type};base64,{data}");
            Some(o::UserPart::ImageUrl {
                image_url: o::ImageUrl { url },
            })
        }
        "url" => src.url.as_ref().map(|u| o::UserPart::ImageUrl {
            image_url: o::ImageUrl { url: u.clone() },
        }),
        _ => None,
    }
}

fn flatten_tool_result(content: &a::ToolResultContent) -> String {
    match content {
        a::ToolResultContent::Text(t) => t.clone(),
        a::ToolResultContent::Blocks(blocks) => {
            // OpenAI expects a plain string here; collapse any nested
            // text blocks. Image / other types are best-effort skipped.
            blocks
                .iter()
                .filter_map(|b| match b {
                    a::ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn simple_user_msg(text: &str) -> a::Message {
        a::Message {
            role: a::Role::User,
            content: a::MessageContent::Text(text.into()),
        }
    }

    #[test]
    fn simple_text_conversation() {
        let req = a::MessagesRequest {
            model: "claude-opus-4-7".into(),
            max_tokens: 1024,
            system: Some(a::SystemPrompt::Text("be brief".into())),
            messages: vec![simple_user_msg("hi")],
            tools: vec![],
            tool_choice: None,
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: Some(false),
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        let out = convert(&req);
        assert_eq!(out.model, "claude-opus-4-7");
        assert_eq!(out.max_tokens, Some(1024));
        assert_eq!(out.temperature, Some(0.7));
        assert_eq!(out.messages.len(), 2);
        match &out.messages[0] {
            o::ChatMessage::System { content } => assert_eq!(content, "be brief"),
            other => panic!("expected System, got {other:?}"),
        }
        match &out.messages[1] {
            o::ChatMessage::User {
                content: o::UserContent::Text(t),
            } => assert_eq!(t, "hi"),
            other => panic!("expected User Text, got {other:?}"),
        }
    }

    #[test]
    fn assistant_tool_use_becomes_openai_tool_calls() {
        let req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![
                a::Message {
                    role: a::Role::Assistant,
                    content: a::MessageContent::Blocks(vec![
                        a::ContentBlock::Text {
                            text: "let me check".into(),
                            cache_control: None,
                        },
                        a::ContentBlock::ToolUse {
                            id: "toolu_1".into(),
                            name: "Read".into(),
                            input: json!({"path": "/tmp/a"}),
                        },
                    ]),
                },
            ],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        let out = convert(&req);
        assert_eq!(out.messages.len(), 1);
        match &out.messages[0] {
            o::ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                assert_eq!(content.as_deref(), Some("let me check"));
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].function.name, "Read");
                let args: serde_json::Value =
                    serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
                assert_eq!(args["path"], "/tmp/a");
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_becomes_role_tool_message() {
        let req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![a::Message {
                role: a::Role::User,
                content: a::MessageContent::Blocks(vec![a::ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: a::ToolResultContent::Text("file contents".into()),
                    is_error: None,
                }]),
            }],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        let out = convert(&req);
        assert_eq!(out.messages.len(), 1);
        match &out.messages[0] {
            o::ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                assert_eq!(tool_call_id, "toolu_1");
                assert_eq!(content, "file contents");
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn mixed_text_and_tool_result_preserves_order() {
        // Anthropic user blocks: [text, tool_result, text]
        // Expected OpenAI messages: [user, tool, user]
        let req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![a::Message {
                role: a::Role::User,
                content: a::MessageContent::Blocks(vec![
                    a::ContentBlock::Text {
                        text: "first".into(),
                        cache_control: None,
                    },
                    a::ContentBlock::ToolResult {
                        tool_use_id: "tr1".into(),
                        content: a::ToolResultContent::Text("result1".into()),
                        is_error: None,
                    },
                    a::ContentBlock::Text {
                        text: "second".into(),
                        cache_control: None,
                    },
                ]),
            }],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        let out = convert(&req);
        assert_eq!(out.messages.len(), 3);
        assert!(matches!(out.messages[0], o::ChatMessage::User { .. }));
        assert!(matches!(out.messages[1], o::ChatMessage::Tool { .. }));
        assert!(matches!(out.messages[2], o::ChatMessage::User { .. }));
    }

    #[test]
    fn thinking_blocks_are_stripped() {
        let req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![a::Message {
                role: a::Role::Assistant,
                content: a::MessageContent::Blocks(vec![
                    a::ContentBlock::Thinking {
                        thinking: "reasoning here".into(),
                        signature: Some("sig".into()),
                    },
                    a::ContentBlock::Text {
                        text: "visible answer".into(),
                        cache_control: None,
                    },
                ]),
            }],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: Some(json!({"type": "enabled"})),
        };
        let out = convert(&req);
        match &out.messages[0] {
            o::ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                assert_eq!(content.as_deref(), Some("visible answer"));
                assert!(tool_calls.is_empty());
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn tools_definitions_rewrap_correctly() {
        let req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![simple_user_msg("hi")],
            tools: vec![a::Tool {
                name: "Read".into(),
                description: Some("Read a file".into()),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        let out = convert(&req);
        assert_eq!(out.tools.len(), 1);
        assert_eq!(out.tools[0].function.name, "Read");
        assert_eq!(out.tools[0].function.description.as_deref(), Some("Read a file"));
        assert_eq!(out.tools[0].function.parameters["type"], "object");
    }

    #[test]
    fn tool_choice_modes_translate_correctly() {
        let make_req = |tc: a::ToolChoice| a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![simple_user_msg("hi")],
            tools: vec![],
            tool_choice: Some(tc),
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec![],
            metadata: None,
            thinking: None,
        };
        assert!(matches!(
            convert(&make_req(a::ToolChoice::Auto)).tool_choice,
            Some(o::ChatToolChoice::Mode(ref s)) if s == "auto"
        ));
        assert!(matches!(
            convert(&make_req(a::ToolChoice::Any)).tool_choice,
            Some(o::ChatToolChoice::Mode(ref s)) if s == "required"
        ));
        assert!(matches!(
            convert(&make_req(a::ToolChoice::Tool { name: "R".into() })).tool_choice,
            Some(o::ChatToolChoice::Specific { ref function, .. }) if function.name == "R"
        ));
    }

    #[test]
    fn stop_sequences_chose_single_vs_array() {
        let mut req = a::MessagesRequest {
            model: "x".into(),
            max_tokens: 100,
            system: None,
            messages: vec![simple_user_msg("hi")],
            tools: vec![],
            tool_choice: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: None,
            stop_sequences: vec!["END".into()],
            metadata: None,
            thinking: None,
        };
        assert!(matches!(convert(&req).stop, Some(o::StopValue::Single(_))));

        req.stop_sequences = vec!["A".into(), "B".into()];
        match convert(&req).stop {
            Some(o::StopValue::Many(v)) => assert_eq!(v.len(), 2),
            other => panic!("expected Many, got {other:?}"),
        }
    }
}
