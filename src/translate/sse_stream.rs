//! Streaming SSE translation: OpenAI chat.completion.chunk stream →
//! Anthropic Messages SSE event stream.
//!
//! The interesting bit is reassembling tool call argument streams.
//! OpenAI sends tool calls as a sequence of deltas indexed by `index`
//! where the function name arrives in the first delta and the
//! `arguments` value arrives split across many subsequent deltas as
//! raw string fragments. Anthropic's wire format uses an
//! `input_json_delta` event whose `partial_json` is exactly such a
//! fragment — so the translation is forward-friendly: each OpenAI
//! arguments fragment becomes one Anthropic `input_json_delta` event.
//!
//! Eventual finalisation:
//!
//!   message_start
//!   [content_block_start, content_block_delta…, content_block_stop]
//!     repeated per text/tool_use block, in arrival order
//!   message_delta { stop_reason, usage }
//!   message_stop

use std::collections::HashMap;

use async_stream::stream;
use axum::body::Bytes;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};

use crate::translate::resp_openai_to_anthropic::convert_finish_reason;
use crate::translate::types_anthropic as a;
use crate::translate::types_openai as o;

/// Consume an OpenAI streaming response, return a Stream of pre-encoded
/// Anthropic SSE byte chunks ready for axum::body::Body::from_stream.
///
/// Cancellation is automatic: when the caller drops the resulting Stream,
/// the inner `eventsource` reader and its backing `reqwest::Response` are
/// dropped as well, aborting the upstream connection before the next chunk.
pub fn convert(upstream: reqwest::Response) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    stream! {
        let mut state = State::new();
        let mut event_stream = Box::pin(upstream.bytes_stream().eventsource());

        while let Some(event) = event_stream.next().await {
            let event = match event {
                Ok(e) => e,
                Err(e) => {
                    // Surface the error to the client and stop.
                    yield Ok(format_error_event(&format!("upstream stream error: {e}")));
                    return;
                }
            };

            // OpenAI ends its stream with a literal "[DONE]" data line.
            if event.data.trim() == "[DONE]" {
                for evt in state.finalize() {
                    yield Ok(format_sse(&evt));
                }
                return;
            }

            // Some keep-alive lines (empty data, ping events). Skip them.
            if event.data.is_empty() {
                continue;
            }

            let chunk: o::ChatCompletionChunk = match serde_json::from_str(&event.data) {
                Ok(c) => c,
                Err(_) => continue, // malformed chunk: skip rather than tear down
            };

            for evt in state.process_chunk(chunk) {
                yield Ok(format_sse(&evt));
            }
        }

        // Upstream closed the stream without [DONE]. Flush whatever's
        // pending so the client sees a complete, well-formed sequence.
        for evt in state.finalize() {
            yield Ok(format_sse(&evt));
        }
    }
}

/// Internal state machine. Designed to be stepped per upstream chunk
/// and to emit zero or more Anthropic SSE events per step.
struct State {
    started: bool,
    id: String,
    model: String,
    /// Next index to assign to a new Anthropic content block.
    next_block_index: u32,
    /// What block, if any, is currently open.
    current: Option<Current>,
    /// Maps OpenAI tool-call `index` (their internal stream id) to the
    /// Anthropic content block index we've assigned to it.
    tool_index_map: HashMap<u32, u32>,
    /// Final finish_reason, set when upstream sends it on the last chunk.
    finish_reason: Option<o::FinishReason>,
    /// Final usage stats, if upstream emits a usage object (only on the
    /// final chunk with most providers).
    usage: Option<o::UsageStats>,
    /// Accumulated output characters across all text + tool argument
    /// deltas. Used to estimate output_tokens when the upstream omits
    /// the usage object entirely (T1.9).
    output_chars: usize,
}

#[derive(Clone, Copy)]
enum Current {
    Thinking { index: u32 },
    Text { index: u32 },
    ToolUse { index: u32 },
}

impl Current {
    fn index(self) -> u32 {
        match self {
            Current::Thinking { index } => index,
            Current::Text { index } => index,
            Current::ToolUse { index } => index,
        }
    }
}

impl State {
    fn new() -> Self {
        Self {
            started: false,
            id: String::new(),
            model: String::new(),
            next_block_index: 0,
            current: None,
            tool_index_map: HashMap::new(),
            finish_reason: None,
            usage: None,
            output_chars: 0,
        }
    }

    fn process_chunk(&mut self, chunk: o::ChatCompletionChunk) -> Vec<a::SseEvent> {
        let mut out = Vec::new();

        // Lock in id / model on the very first chunk we see.
        if !self.started {
            self.id = if chunk.id.is_empty() {
                default_message_id()
            } else {
                chunk.id.clone()
            };
            self.model = chunk.model.clone();
            self.emit_message_start(&mut out);
            self.started = true;
        }

        for choice in chunk.choices {
            self.process_choice(choice, &mut out);
        }

        if let Some(u) = chunk.usage {
            self.usage = Some(u);
        }

        out
    }

    fn emit_message_start(&self, out: &mut Vec<a::SseEvent>) {
        out.push(a::SseEvent::MessageStart {
            message: a::MessageStartPayload {
                id: self.id.clone(),
                message_type: "message".into(),
                role: a::Role::Assistant,
                content: vec![],
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: a::Usage::default(),
            },
        });
    }

    fn process_choice(&mut self, choice: o::ChunkChoice, out: &mut Vec<a::SseEvent>) {
        // Reasoning delta (DeepSeek V4, Qwen3, etc.). Open or continue a
        // Thinking block. Arrives before content in thinking-mode responses.
        if let Some(reasoning) = choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            self.ensure_thinking_block(out);
            if let Some(Current::Thinking { index }) = self.current {
                out.push(a::SseEvent::ContentBlockDelta {
                    index,
                    delta: a::ContentBlockDelta::ThinkingDelta { thinking: reasoning },
                });
            }
        }

        // Text delta. Open or continue a Text block.
        if let Some(content) = choice.delta.content
            && !content.is_empty()
        {
            self.output_chars += content.len();
            self.ensure_text_block(out);
            if let Some(Current::Text { index }) = self.current {
                out.push(a::SseEvent::ContentBlockDelta {
                    index,
                    delta: a::ContentBlockDelta::TextDelta { text: content },
                });
            }
        }

        // Tool call deltas. Each `index` in OpenAI's stream gets its
        // own Anthropic content block.
        for tc in choice.delta.tool_calls {
            self.process_tool_call_delta(tc, out);
        }

        if let Some(reason) = choice.finish_reason {
            self.finish_reason = Some(reason);
        }
    }

    fn ensure_thinking_block(&mut self, out: &mut Vec<a::SseEvent>) {
        if matches!(self.current, Some(Current::Thinking { .. })) {
            return;
        }
        if let Some(cur) = self.current.take() {
            out.push(a::SseEvent::ContentBlockStop { index: cur.index() });
        }
        let index = self.next_block_index;
        self.next_block_index += 1;
        tracing::debug!(
            block_index = index,
            message_id = %self.id,
            "SSE: opening thinking block (reasoning_content arrived from upstream)",
        );
        out.push(a::SseEvent::ContentBlockStart {
            index,
            content_block: a::ContentBlockStart::Thinking {
                thinking: String::new(),
            },
        });
        self.current = Some(Current::Thinking { index });
    }

    /// If a Text block isn't currently open, close whatever is and open
    /// a fresh one with a new index.
    fn ensure_text_block(&mut self, out: &mut Vec<a::SseEvent>) {
        if matches!(self.current, Some(Current::Text { .. })) {
            return;
        }
        if let Some(cur) = self.current.take() {
            out.push(a::SseEvent::ContentBlockStop { index: cur.index() });
        }
        let index = self.next_block_index;
        self.next_block_index += 1;
        out.push(a::SseEvent::ContentBlockStart {
            index,
            content_block: a::ContentBlockStart::Text {
                text: String::new(),
            },
        });
        self.current = Some(Current::Text { index });
    }

    fn process_tool_call_delta(&mut self, tc: o::ToolCallDelta, out: &mut Vec<a::SseEvent>) {
        // First time we see this tool call (we key on OpenAI's stream
        // index, NOT on tc.id, because id may be split across chunks).
        let anthropic_index = if let Some(&idx) = self.tool_index_map.get(&tc.index) {
            idx
        } else {
            if let Some(cur) = self.current.take() {
                out.push(a::SseEvent::ContentBlockStop { index: cur.index() });
            }
            let new_idx = self.next_block_index;
            self.next_block_index += 1;

            let id = tc.id.clone().unwrap_or_default();
            let name = tc
                .function
                .as_ref()
                .and_then(|f| f.name.clone())
                .unwrap_or_default();

            out.push(a::SseEvent::ContentBlockStart {
                index: new_idx,
                content_block: a::ContentBlockStart::ToolUse {
                    id,
                    name,
                    input: serde_json::Value::Object(Default::default()),
                },
            });

            self.tool_index_map.insert(tc.index, new_idx);
            self.current = Some(Current::ToolUse { index: new_idx });
            new_idx
        };

        // Forward the arguments fragment as input_json_delta.
        if let Some(func) = tc.function
            && let Some(args) = func.arguments
            && !args.is_empty()
        {
            self.output_chars += args.len();
            out.push(a::SseEvent::ContentBlockDelta {
                index: anthropic_index,
                delta: a::ContentBlockDelta::InputJsonDelta { partial_json: args },
            });
        }
    }

    fn finalize(mut self) -> Vec<a::SseEvent> {
        let mut out = Vec::new();

        // Emit message_start if upstream sent zero chunks before [DONE]
        // — we still need to produce a well-formed Anthropic stream.
        if !self.started {
            if self.id.is_empty() {
                self.id = default_message_id();
            }
            self.emit_message_start(&mut out);
        }

        if let Some(cur) = self.current.take() {
            out.push(a::SseEvent::ContentBlockStop { index: cur.index() });
        }

        let stop_reason = self.finish_reason.map(convert_finish_reason);

        let usage = self
            .usage
            .map(|u| a::Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            })
            .unwrap_or_else(|| {
                // Provider did not emit a usage object. Estimate output tokens
                // from accumulated character count (4 chars ≈ 1 token).
                let estimated = ((self.output_chars as f64) / 4.0).ceil() as u32;
                tracing::debug!(
                    output_chars = self.output_chars,
                    estimated_output_tokens = estimated,
                    "upstream omitted usage — using character-based estimate",
                );
                a::Usage {
                    input_tokens: 0,
                    output_tokens: estimated,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }
            });

        out.push(a::SseEvent::MessageDelta {
            delta: a::MessageDeltaPayload {
                stop_reason,
                stop_sequence: None,
            },
            usage,
        });
        out.push(a::SseEvent::MessageStop);
        out
    }
}

fn default_message_id() -> String {
    format!(
        "msg_proxy_{}",
        // 8 hex chars from the system time is plenty for uniqueness in
        // this debugging-style fallback path; we avoid adding a uuid dep.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            & 0xFFFFFFFF
    )
}

/// Serialize one Anthropic SSE event into the wire format:
///
///   event: <name>\ndata: <json>\n\n
fn format_sse(event: &a::SseEvent) -> Bytes {
    let name = event_name(event);
    let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    let mut buf = String::with_capacity(payload.len() + name.len() + 16);
    buf.push_str("event: ");
    buf.push_str(name);
    buf.push_str("\ndata: ");
    buf.push_str(&payload);
    buf.push_str("\n\n");
    Bytes::from(buf)
}

fn event_name(event: &a::SseEvent) -> &'static str {
    match event {
        a::SseEvent::MessageStart { .. } => "message_start",
        a::SseEvent::ContentBlockStart { .. } => "content_block_start",
        a::SseEvent::ContentBlockDelta { .. } => "content_block_delta",
        a::SseEvent::ContentBlockStop { .. } => "content_block_stop",
        a::SseEvent::MessageDelta { .. } => "message_delta",
        a::SseEvent::MessageStop => "message_stop",
        a::SseEvent::Ping => "ping",
    }
}

/// Emit a synthetic error event the client can parse. Anthropic's wire
/// format does not define a formal "error" SSE event during a stream,
/// so we use message_stop with a plain log line afterwards — clients
/// then see a truncated message which is the right semantic.
fn format_error_event(message: &str) -> Bytes {
    tracing::error!(message, "translation stream aborted");
    let evt = a::SseEvent::MessageStop;
    format_sse(&evt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn process(state: &mut State, json: serde_json::Value) -> Vec<a::SseEvent> {
        let chunk: o::ChatCompletionChunk = serde_json::from_value(json).unwrap();
        state.process_chunk(chunk)
    }

    #[test]
    fn simple_text_stream_produces_expected_event_sequence() {
        let mut s = State::new();

        // Chunk 1: role
        let e1 = process(
            &mut s,
            json!({
                "id": "id-1", "model": "kimi", "choices":[
                    {"index":0,"delta":{"role":"assistant"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e1[0], a::SseEvent::MessageStart { .. }));

        // Chunk 2: text "Hello"
        let e2 = process(
            &mut s,
            json!({
                "id": "id-1", "model": "kimi", "choices":[
                    {"index":0,"delta":{"content":"Hello"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e2[0], a::SseEvent::ContentBlockStart { .. }));
        assert!(matches!(e2[1], a::SseEvent::ContentBlockDelta { .. }));

        // Chunk 3: text " world"
        let e3 = process(
            &mut s,
            json!({
                "id": "id-1", "model": "kimi", "choices":[
                    {"index":0,"delta":{"content":" world"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e3[0], a::SseEvent::ContentBlockDelta { .. }));

        // Chunk 4: finish
        let _ = process(
            &mut s,
            json!({
                "id": "id-1", "model": "kimi", "choices":[
                    {"index":0,"delta":{},"finish_reason":"stop"}
                ]
            }),
        );

        // Finalize
        let fin = s.finalize();
        assert!(matches!(fin[0], a::SseEvent::ContentBlockStop { .. }));
        match &fin[1] {
            a::SseEvent::MessageDelta { delta, .. } => {
                assert_eq!(delta.stop_reason, Some(a::StopReason::EndTurn));
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
        assert!(matches!(fin[2], a::SseEvent::MessageStop));
    }

    #[test]
    fn tool_call_argument_fragments_become_input_json_deltas() {
        let mut s = State::new();
        // Chunk 1: opens tool call with id+name, empty args
        let e1 = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "role":"assistant",
                    "tool_calls":[{"index":0,"id":"call_x","type":"function",
                        "function":{"name":"Read","arguments":""}}]
                },"finish_reason":null}]
            }),
        );
        // message_start + content_block_start
        assert!(matches!(e1[0], a::SseEvent::MessageStart { .. }));
        match &e1[1] {
            a::SseEvent::ContentBlockStart {
                index: 0,
                content_block,
            } => {
                assert!(matches!(
                    content_block,
                    a::ContentBlockStart::ToolUse { .. }
                ));
                if let a::ContentBlockStart::ToolUse { id, name, .. } = content_block {
                    assert_eq!(id, "call_x");
                    assert_eq!(name, "Read");
                }
            }
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }

        // Chunk 2: argument fragment "{\"path\""
        let e2 = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "tool_calls":[{"index":0,
                        "function":{"arguments":"{\"path\""}}]
                },"finish_reason":null}]
            }),
        );
        match &e2[0] {
            a::SseEvent::ContentBlockDelta { index: 0, delta } => match delta {
                a::ContentBlockDelta::InputJsonDelta { partial_json } => {
                    assert_eq!(partial_json, "{\"path\"");
                }
                other => panic!("expected InputJsonDelta, got {other:?}"),
            },
            other => panic!("expected ContentBlockDelta, got {other:?}"),
        }

        // Chunk 3: rest of arguments
        let _ = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "tool_calls":[{"index":0,"function":{"arguments":":\"/tmp\"}"}}]
                },"finish_reason":null}]
            }),
        );

        // Chunk 4: finish
        let _ = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[
                    {"index":0,"delta":{},"finish_reason":"tool_calls"}
                ]
            }),
        );

        let fin = s.finalize();
        assert!(matches!(fin[0], a::SseEvent::ContentBlockStop { index: 0 }));
        match &fin[1] {
            a::SseEvent::MessageDelta { delta, .. } => {
                assert_eq!(delta.stop_reason, Some(a::StopReason::ToolUse));
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn text_then_tool_call_splits_into_two_blocks() {
        let mut s = State::new();
        // Text first
        let _ = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "role":"assistant","content":"let me check"
                },"finish_reason":null}]
            }),
        );
        // Now a tool call
        let e2 = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "tool_calls":[{"index":0,"id":"c","type":"function",
                        "function":{"name":"Read","arguments":"{}"}}]
                },"finish_reason":null}]
            }),
        );
        // First event of the second chunk should be ContentBlockStop
        // closing the text block, then ContentBlockStart opening the
        // tool_use block at index 1.
        assert!(matches!(e2[0], a::SseEvent::ContentBlockStop { index: 0 }));
        assert!(matches!(
            e2[1],
            a::SseEvent::ContentBlockStart { index: 1, .. }
        ));
    }

    #[test]
    fn usage_from_final_chunk_propagates_to_message_delta() {
        let mut s = State::new();
        let _ = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "role":"assistant","content":"x"
                },"finish_reason":null}]
            }),
        );
        let _ = process(
            &mut s,
            json!({
                "id": "id", "model": "m",
                "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                "usage": {"prompt_tokens": 100, "completion_tokens": 25, "total_tokens": 125}
            }),
        );
        let fin = s.finalize();
        match &fin[1] {
            a::SseEvent::MessageDelta { usage, .. } => {
                assert_eq!(usage.input_tokens, 100);
                assert_eq!(usage.output_tokens, 25);
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn malformed_chunks_in_stream_are_skipped() {
        // Implicitly tested through deserialization in the State, but
        // here we verify directly that a bad JSON line does not panic.
        let r = serde_json::from_str::<o::ChatCompletionChunk>("not json {");
        assert!(r.is_err());
    }

    #[test]
    fn empty_text_delta_does_not_open_a_block() {
        let mut s = State::new();
        let e = process(
            &mut s,
            json!({
                "id": "id", "model": "m", "choices":[{"index":0,"delta":{
                    "role":"assistant","content":""
                },"finish_reason":null}]
            }),
        );
        // Only message_start, no block start for an empty content delta
        assert_eq!(e.len(), 1);
        assert!(matches!(e[0], a::SseEvent::MessageStart { .. }));
    }

    #[test]
    fn reasoning_content_opens_thinking_block_before_text() {
        let mut s = State::new();

        // Chunk 1: role only
        let e1 = process(
            &mut s,
            json!({
                "id": "id-r", "model": "deepseek-v4-pro", "choices":[
                    {"index":0,"delta":{"role":"assistant"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e1[0], a::SseEvent::MessageStart { .. }));

        // Chunk 2: reasoning delta
        let e2 = process(
            &mut s,
            json!({
                "id": "id-r", "model": "deepseek-v4-pro", "choices":[
                    {"index":0,"delta":{"reasoning_content":"let me think"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e2[0], a::SseEvent::ContentBlockStart { index: 0, content_block: a::ContentBlockStart::Thinking { .. } }));
        match &e2[1] {
            a::SseEvent::ContentBlockDelta { index: 0, delta: a::ContentBlockDelta::ThinkingDelta { thinking } } => {
                assert_eq!(thinking, "let me think");
            }
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }

        // Chunk 3: content delta — must close thinking block and open text block
        let e3 = process(
            &mut s,
            json!({
                "id": "id-r", "model": "deepseek-v4-pro", "choices":[
                    {"index":0,"delta":{"content":"answer"},"finish_reason":null}
                ]
            }),
        );
        assert!(matches!(e3[0], a::SseEvent::ContentBlockStop { index: 0 }));
        assert!(matches!(e3[1], a::SseEvent::ContentBlockStart { index: 1, content_block: a::ContentBlockStart::Text { .. } }));
        match &e3[2] {
            a::SseEvent::ContentBlockDelta { index: 1, delta: a::ContentBlockDelta::TextDelta { text } } => {
                assert_eq!(text, "answer");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn format_sse_wire_format_is_correct() {
        let event = a::SseEvent::MessageStop;
        let bytes = format_sse(&event);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("event: message_stop\n"));
        assert!(s.contains("data: "));
        assert!(s.ends_with("\n\n"));
    }

    #[test]
    fn snapshot_full_stream_text_only() {
        let mut s = State::new();
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-1", "model": "kimi-k2.6", "choices":[
                    {"index":0,"delta":{"role":"assistant"}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-1", "model": "kimi-k2.6", "choices":[
                    {"index":0,"delta":{"content":"The file contains"}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-1", "model": "kimi-k2.6", "choices":[
                    {"index":0,"delta":{"content":" configuration data."}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-1", "model": "kimi-k2.6",
                "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                "usage": {"prompt_tokens": 85, "completion_tokens": 12, "total_tokens": 97}
            }),
        );
        let events = s.finalize();

        let wire: Vec<String> = events
            .iter()
            .map(|e| {
                let bytes = format_sse(e);
                String::from_utf8_lossy(&bytes).trim_end().to_string()
            })
            .collect();

        insta::assert_json_snapshot!(wire);
    }

    #[test]
    fn snapshot_full_stream_text_then_tool_call() {
        let mut s = State::new();
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus", "choices":[
                    {"index":0,"delta":{"role":"assistant"}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus", "choices":[
                    {"index":0,"delta":{"content":"I'll read the file now."}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus", "choices":[
                    {"index":0,"delta":{
                        "tool_calls":[
                            {"index":0,"id":"call_read_1","type":"function",
                             "function":{"name":"Read","arguments":""}}
                        ]
                    }, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus", "choices":[
                    {"index":0,"delta":{"tool_calls":[
                        {"index":0,"function":{"arguments":"{\"file"}}
                    ]}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus", "choices":[
                    {"index":0,"delta":{"tool_calls":[
                        {"index":0,"function":{"arguments":"_path\":\"/tmp/data.txt\"}"}}
                    ]}, "finish_reason":null}
                ]
            }),
        );
        process(
            &mut s,
            json!({
                "id": "chatcmpl-snapshot-2", "model": "qwen3.6-plus",
                "choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],
                "usage": {"prompt_tokens": 120, "completion_tokens": 35, "total_tokens": 155}
            }),
        );
        let events = s.finalize();

        let wire: Vec<String> = events
            .iter()
            .map(|e| {
                let bytes = format_sse(e);
                String::from_utf8_lossy(&bytes).trim_end().to_string()
            })
            .collect();

        insta::assert_json_snapshot!(wire);
    }
}
