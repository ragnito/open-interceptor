//! OpenAI-compatible provider (translation layer).
//!
//! Pipeline (non-streaming, T3.7 partial):
//!   1. parse the incoming body as an Anthropic MessagesRequest
//!   2. apply the route's effective_model rewrite
//!   3. translate to OpenAI ChatCompletionRequest
//!   4. POST to <provider.url>/v1/chat/completions with the configured
//!      API key as `Authorization: Bearer ...`
//!   5. parse the OpenAI response, translate back to Anthropic shape
//!   6. return JSON to the client
//!
//! Streaming (`stream: true`) is rejected with an explicit error today
//! and lands in T3.5.

use std::sync::OnceLock;

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::Response,
};
use reqwest::Client;

use crate::domain::config::Provider;
use crate::providers::truncate_for_log;
use crate::translate::{
    req_anthropic_to_openai, resp_openai_to_anthropic, sse_stream, types_anthropic, types_openai,
};

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

fn http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest client init")
    })
}

pub async fn forward(
    provider: &Provider,
    api_key: Option<&str>,
    _request_model: &str,
    effective_model: &str,
    body: Bytes,
) -> Result<Response, ForwardError> {
    // 0. Normalize any role:"system" messages inside the messages array
    //    to the top-level system field. Some clients (newer Claude Code
    //    versions, etc.) place the system prompt as a message entry
    //    instead of using the canonical top-level field.
    let body = normalize_system_messages(&body);

    // 1. Parse Anthropic request.
    let mut a_req: types_anthropic::MessagesRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            tracing::warn!(
                parse_error = %e,
                body_preview = %String::from_utf8_lossy(&body).chars().take(500).collect::<String>(),
                "failed to parse request body as Anthropic MessagesRequest",
            );
            return Err(ForwardError::RequestParse(e));
        }
    };

    // 2. Route-level remap.
    if a_req.model != effective_model {
        a_req.model = effective_model.to_string();
    }

    // 3. Convert to OpenAI request. We honour the original stream flag —
    // OpenAI also supports SSE streaming, so we ask upstream for the
    // same shape the client wants and translate per-chunk on the way
    // back.
    let wants_stream = a_req.stream.unwrap_or(false);
    let oai_req = req_anthropic_to_openai::convert(&a_req);
    let oai_body = serde_json::to_vec(&oai_req).map_err(ForwardError::RequestSerialize)?;

    // 4. POST upstream.
    let upstream_url = build_upstream_url(&provider.url);
    let key = api_key.ok_or(ForwardError::MissingApiKey)?;

    tracing::debug!(
        url = %upstream_url,
        headers = %format!(
            "authorization: Bearer [REDACTED] | content-type: application/json | accept: {}",
            if wants_stream { "text/event-stream" } else { "application/json" }
        ),
        body_bytes = oai_body.len(),
        body_preview = %truncate_for_log(&oai_body, 512),
        "→ upstream OpenAI request",
    );

    let upstream_resp = http_client()
        .post(&upstream_url)
        .bearer_auth(key)
        .header("content-type", "application/json")
        // Match the Accept the client implicitly wants; some upstreams
        // require it to actually emit text/event-stream.
        .header(
            "accept",
            if wants_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(oai_body)
        .send()
        .await
        .map_err(ForwardError::Upstream)?;

    let status = upstream_resp.status();

    if !status.is_success() {
        let body = upstream_resp
            .bytes()
            .await
            .map_err(ForwardError::Upstream)?;
        let body_text = String::from_utf8_lossy(&body).into_owned();

        if status.as_u16() == 429
            || (status.as_u16() == 403 && body_text.to_lowercase().contains("quota"))
        {
            return Err(ForwardError::RateLimited);
        }

        // Diagnostic dump: the last 3 messages we sent upstream so we can
        // see whether the assistant turn included reasoning_content,
        // tool_calls, etc. Full request body on a separate DEBUG line.
        let tail = summarize_message_tail(&oai_req.messages, 3);
        tracing::debug!(
            upstream_status = status.as_u16(),
            upstream_body = %body_text,
            message_count = oai_req.messages.len(),
            tail_shape = %tail,
            "upstream error — dumping recent message shape",
        );
        let oai_req_serialized = serde_json::to_vec(&oai_req).unwrap_or_default();
        tracing::debug!(
            upstream_request_body_bytes = oai_req_serialized.len(),
            upstream_request_body_preview = %truncate_for_log(&oai_req_serialized, 512),
            "upstream error — full request body",
        );

        return Err(ForwardError::UpstreamError {
            status: status.as_u16(),
            body: body_text,
        });
    }

    if wants_stream {
        // 5a. Streaming path: the client receives Anthropic-shaped SSE
        //     events chunk-by-chunk with no buffering.
        //
        //     Cancellation: when the client disconnects, Axum drops the
        //     Body and stops polling the stream. This drops
        //     sse_stream::convert()'s internal reader, which drops the
        //     reqwest Response, aborting the upstream connection before
        //     the next chunk arrives. No explicit CancellationToken is
        //     needed — Rust's ownership model propagates cancellation
        //     through the async call stack automatically.
        let stream = sse_stream::convert(upstream_resp);
        let body = Body::from_stream(stream);
        Ok(axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .expect("valid response"))
    } else {
        // 5b. Non-streaming path: parse, translate, serialize.
        let oai_resp: types_openai::ChatCompletionResponse =
            upstream_resp.json().await.map_err(ForwardError::Upstream)?;
        let a_resp = resp_openai_to_anthropic::convert_non_streaming(&oai_resp);
        let out_body = serde_json::to_vec(&a_resp).map_err(ForwardError::ResponseSerialize)?;
        Ok(axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(out_body))
            .expect("valid response"))
    }
}

/// Pre-process the request body to handle `role: "system"` messages that
/// some clients place inside `messages[]` instead of the top-level `system`
/// field. Anthropic's Messages API specifies the system prompt in a
/// top-level field, but some clients emit it as a message entry.
fn normalize_system_messages(body: &Bytes) -> Bytes {
    let mut value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return body.clone(),
    };

    let messages = match value.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(arr) => arr,
        None => return body.clone(),
    };

    let mut system_texts: Vec<String> = Vec::new();
    let mut i = messages.len();
    while i > 0 {
        i -= 1;
        if let Some(role) = messages[i].get("role").and_then(|r| r.as_str())
            && role == "system"
        {
            if let Some(text) = extract_system_content(&messages[i]) {
                system_texts.push(text);
            }
            messages.remove(i);
        }
    }

    if system_texts.is_empty() {
        return body.clone();
    }

    system_texts.reverse();
    let combined = system_texts.join("\n");

    let final_text = match value.get("system") {
        Some(serde_json::Value::String(s)) => format!("{s}\n\n{combined}"),
        Some(_) => combined.clone(),
        None => combined,
    };

    value["system"] = serde_json::Value::String(final_text);

    tracing::debug!(
        found_system_messages = system_texts.len(),
        "normalized role:system messages to top-level system field"
    );

    Bytes::from(serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec()))
}

/// Extract the text content from a system message, handling both string
/// and array-of-blocks shapes.
fn extract_system_content(msg: &serde_json::Value) -> Option<String> {
    match msg.get("content")? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let texts: Vec<String> = arr
                .iter()
                .filter_map(|block| {
                    let t = block.get("type")?.as_str()?;
                    if t == "text" {
                        Some(block.get("text")?.as_str()?.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

/// Compact, allocation-cheap summary of the last N messages — role, content
/// length, presence of reasoning_content, and tool_calls metadata. Used on
/// the upstream-error path so we can diagnose what the model saw without
/// dumping prompts verbatim into the logs.
pub fn summarize_message_tail(messages: &[types_openai::ChatMessage], n: usize) -> String {
    let start = messages.len().saturating_sub(n);
    let mut out = String::new();
    out.push('[');
    for (i, msg) in messages.iter().enumerate().skip(start) {
        if i > start {
            out.push_str(", ");
        }
        match msg {
            types_openai::ChatMessage::System { content } => {
                out.push_str(&format!("#{i} system(len={})", content.len()));
            }
            types_openai::ChatMessage::User { content } => {
                let len = match content {
                    types_openai::UserContent::Text(t) => t.len(),
                    types_openai::UserContent::Parts(p) => p.len(),
                };
                out.push_str(&format!("#{i} user(parts_or_chars={len})"));
            }
            types_openai::ChatMessage::Assistant {
                content,
                tool_calls,
                reasoning_content,
            } => {
                out.push_str(&format!(
                    "#{i} assistant(content_len={}, tool_calls={}, reasoning={})",
                    content.as_deref().map(|c| c.len()).unwrap_or(0),
                    tool_calls.len(),
                    reasoning_content
                        .as_deref()
                        .map(|r| format!("Some(len={})", r.len()))
                        .unwrap_or_else(|| "None".into()),
                ));
            }
            types_openai::ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                out.push_str(&format!(
                    "#{i} tool(id={tool_call_id}, content_len={})",
                    content.len()
                ));
            }
        }
    }
    out.push(']');
    out
}

pub fn build_upstream_url(base: &str) -> String {
    format!("{}/chat/completions", base.trim_end_matches('/'))
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("could not parse Anthropic request body: {0}")]
    RequestParse(#[source] serde_json::Error),

    #[error("could not serialize OpenAI request: {0}")]
    RequestSerialize(#[source] serde_json::Error),

    #[error("could not serialize Anthropic response: {0}")]
    ResponseSerialize(#[source] serde_json::Error),

    #[error("upstream request failed: {0}")]
    Upstream(#[source] reqwest::Error),

    #[error("upstream returned {status}: {body}")]
    UpstreamError { status: u16, body: String },

    #[error("provider needs an api_key configured")]
    MissingApiKey,

    #[error("upstream returned 429 rate limit or quota exceeded")]
    RateLimited,
}
