//! Anthropic-compatible upstream provider.
//!
//! This is the most sensitive code path in the proxy. When
//! `passthrough_auth: true` is configured (the OAuth-subscription case),
//! the request that leaves the proxy must be byte-identical, at the
//! application layer, to one Claude Code would have sent direct. See
//! `docs/claude-code-headers.md` for the empirical contract.
//!
//! Specifically:
//!
//! - Path (including query string like `?beta=true`) is preserved.
//! - End-to-end headers are forwarded verbatim. Hop-by-hop headers
//!   (RFC 7230 §6.1) are dropped — they describe the per-hop TCP
//!   connection, not the request. Same for `Host`, which reqwest fills
//!   in correctly for the new upstream connection.
//! - The request body is forwarded byte-for-byte unless a route
//!   `remap` rewrote the model id. The body carries `metadata.user_id`
//!   with device/account/session identifiers — tampering would be a
//!   "there is a proxy here" signal.
//! - No proxy-disclosing headers (`Via`, `X-Forwarded-*`, `Forwarded`)
//!   are added.
//! - The upstream response, including SSE streaming bodies, is relayed
//!   chunk-by-chunk to the client. No buffering, no re-encoding, no
//!   decompression.

use std::sync::OnceLock;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use reqwest::Client;

use crate::domain::config::Provider;

/// Lazily-initialized shared `reqwest::Client`. Connection pooling and
/// keep-alive happen here, so creating one per request is wasteful.
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

fn http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        // We deliberately do NOT set a default User-Agent: in
        // passthrough_auth mode we want the client's User-Agent to be
        // visible upstream, not ours. The default reqwest UA is
        // overwritten per-request by `build_upstream_headers` anyway.
        Client::builder()
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest client init")
    })
}

/// Forward a request to an Anthropic-compatible upstream and stream the
/// response back. Pure I/O — error mapping to client-facing responses
/// happens in the proxy layer.
#[allow(clippy::too_many_arguments)]
pub async fn forward(
    provider: &Provider,
    api_key: Option<&str>,
    request_model: &str,
    effective_model: &str,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ForwardError> {
    let upstream_url = build_upstream_url(&provider.url, &uri);

    let upstream_body = sanitize_body(&body, effective_model, request_model)?;

    let upstream_headers = build_upstream_headers(&headers, provider, api_key)?;

    let response = http_client()
        .request(method, &upstream_url)
        .headers(upstream_headers)
        .body(upstream_body)
        .send()
        .await
        .map_err(ForwardError::Upstream)?;

    if response.status() == 429 {
        return Err(ForwardError::RateLimited);
    }

    Ok(relay_response(response))
}

/// Concatenate the provider's base URL with the incoming request's path
/// and query string. Trailing slashes are normalized, and a trailing
/// `/v1` is stripped so we don't produce `//v1/messages` or `/v1/v1/messages`
/// when the user configured the base URL with `/v1` already on it.
pub fn build_upstream_url(base: &str, uri: &Uri) -> String {
    let base = base.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let path_and_query = uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/v1/messages");
    format!("{base}{path_and_query}")
}

/// Build the header set we send upstream. End-to-end headers are
/// preserved exactly; hop-by-hop are dropped. The Authorization header
/// is rewritten with the provider's API key unless `passthrough_auth`
/// is on, in which case the client's auth header rides through.
pub fn build_upstream_headers(
    incoming: &HeaderMap,
    provider: &Provider,
    api_key: Option<&str>,
) -> Result<HeaderMap, ForwardError> {
    let mut out = HeaderMap::with_capacity(incoming.len());

    for (name, value) in incoming.iter() {
        let lname = name.as_str();

        if is_hop_by_hop(lname) {
            continue;
        }

        if !provider.passthrough_auth && is_auth_header(lname) {
            continue;
        }

        out.insert(name.clone(), value.clone());
    }

    if !provider.passthrough_auth {
        let key = api_key.ok_or(ForwardError::MissingApiKey)?;
        out.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(key).map_err(ForwardError::InvalidHeaderValue)?,
        );
    }

    Ok(out)
}

/// Hop-by-hop headers per RFC 7230 §6.1 plus `Host` (reqwest sets the
/// correct one for the new TCP connection automatically).
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "host"
            | "content-length" // reqwest recomputes this from the body
    )
}

fn is_auth_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "x-api-key"
    )
}

/// Sanitize the request body before forwarding to Anthropic:
/// 1. Rewrite the `model` field if a route remap applies.
/// 2. Strip Thinking/RedactedThinking blocks from message content.
///    These are generated by the proxy's OpenAI→Anthropic translation
///    and carry fake signatures that Anthropic would reject.
pub fn sanitize_body(
    body: &Bytes,
    effective_model: &str,
    request_model: &str,
) -> Result<Bytes, ForwardError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(body).map_err(ForwardError::BodyParse)?;

    // 1. Model remap.
    if effective_model != request_model
        && let Some(obj) = value.as_object_mut()
    {
        obj.insert(
            "model".to_string(),
            serde_json::Value::String(effective_model.to_string()),
        );
    }

    // 2. Strip Thinking/RedactedThinking from messages.content.
    if let Some(messages) = value.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for msg in messages.iter_mut() {
            if let Some(content) = msg.get_mut("content") {
                strip_thinking_blocks(content);
            }
        }
    }

    let bytes = serde_json::to_vec(&value).map_err(ForwardError::BodySerialize)?;
    Ok(Bytes::from(bytes))
}

/// Remove Thinking and RedactedThinking blocks from a content array.
/// Works on both array-of-objects and string content (string is left alone).
pub fn strip_thinking_blocks(content: &mut serde_json::Value) {
    if let Some(blocks) = content.as_array_mut() {
        let before = blocks.len();
        blocks.retain(|block| {
            let t = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            t != "thinking" && t != "redacted_thinking"
        });
        let removed = before - blocks.len();
        if removed > 0 {
            tracing::debug!(
                removed,
                "stripped Thinking blocks before forwarding to Anthropic"
            );
        }
    }
}

/// Wrap the upstream `reqwest::Response` in an `axum::Response` whose
/// body is a streaming wrapper. Each chunk is forwarded to the client
/// as it arrives — no buffering. This is the whole reason we picked
/// Rust + Tokio for this layer.
fn relay_response(upstream: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let mut builder = axum::http::Response::builder().status(status);

    // Forward upstream headers, again skipping hop-by-hop. We do NOT
    // touch `content-encoding`: the body stays compressed (gzip/br/zstd)
    // and so does its byte stream.
    for (name, value) in upstream.headers().iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }

    let stream = upstream.bytes_stream();
    let body = Body::from_stream(stream);

    builder
        .body(body)
        .expect("response builder cannot fail with a valid status + body")
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("upstream request failed: {0}")]
    Upstream(#[source] reqwest::Error),

    #[error("provider has neither passthrough_auth nor api_key set")]
    MissingApiKey,

    #[error("upstream returned 429 rate limit")]
    RateLimited,

    #[error("could not parse request body as JSON for remap: {0}")]
    BodyParse(#[source] serde_json::Error),

    #[error("could not re-serialize JSON body after remap: {0}")]
    BodySerialize(#[source] serde_json::Error),

    #[error("invalid header value while building upstream request: {0}")]
    InvalidHeaderValue(axum::http::header::InvalidHeaderValue),
}
