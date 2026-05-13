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

use crate::config::Provider;
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
    _request_model: &str,
    effective_model: &str,
    body: Bytes,
) -> Result<Response, ForwardError> {
    // 1. Parse Anthropic request.
    let mut a_req: types_anthropic::MessagesRequest =
        serde_json::from_slice(&body).map_err(ForwardError::RequestParse)?;

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
    let api_key = provider.api_key.as_deref().ok_or(ForwardError::MissingApiKey)?;

    let upstream_resp = http_client()
        .post(&upstream_url)
        .bearer_auth(api_key)
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
        let body = upstream_resp.bytes().await.map_err(ForwardError::Upstream)?;
        return Err(ForwardError::UpstreamError {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }

    if wants_stream {
        // 5a. Streaming path: hand the upstream Response off to the SSE
        //     translator and wrap the resulting Bytes stream in an axum
        //     Body. The client will see Anthropic-shaped events arriving
        //     chunk-by-chunk with no buffering beyond OS TCP windows.
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

fn build_upstream_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/v1/chat/completions")
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_url_concat() {
        assert_eq!(
            build_upstream_url("https://opencode.ai/zen/go"),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
        assert_eq!(
            build_upstream_url("https://opencode.ai/zen/go/"),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
    }
}
