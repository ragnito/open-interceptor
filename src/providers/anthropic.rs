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

use crate::config::Provider;

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
pub async fn forward(
    provider: &Provider,
    request_model: &str,
    effective_model: &str,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ForwardError> {
    let upstream_url = build_upstream_url(&provider.url, &uri);
    let upstream_body = if effective_model != request_model {
        rewrite_model(&body, effective_model)?
    } else {
        body
    };
    let upstream_headers = build_upstream_headers(&headers, provider)?;

    let response = http_client()
        .request(method, &upstream_url)
        .headers(upstream_headers)
        .body(upstream_body)
        .send()
        .await
        .map_err(ForwardError::Upstream)?;

    Ok(relay_response(response))
}

/// Concatenate the provider's base URL with the incoming request's path
/// and query string. Trailing slashes on the base are normalized so we
/// don't end up with `//v1/messages`.
fn build_upstream_url(base: &str, uri: &Uri) -> String {
    let base = base.trim_end_matches('/');
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
fn build_upstream_headers(
    incoming: &HeaderMap,
    provider: &Provider,
) -> Result<HeaderMap, ForwardError> {
    let mut out = HeaderMap::with_capacity(incoming.len());

    for (name, value) in incoming.iter() {
        let lname = name.as_str();

        if is_hop_by_hop(lname) {
            continue;
        }

        // If we're substituting auth, drop the client's auth header so
        // we don't send two competing keys.
        if !provider.passthrough_auth && is_auth_header(lname) {
            continue;
        }

        out.insert(name.clone(), value.clone());
    }

    if !provider.passthrough_auth {
        let api_key = provider
            .api_key
            .as_deref()
            .ok_or(ForwardError::MissingApiKey)?;
        // x-api-key is the canonical header for native Anthropic and for
        // every Anthropic-compatible upstream we currently target
        // (DeepSeek's /anthropic, etc.).
        out.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(api_key).map_err(ForwardError::InvalidHeaderValue)?,
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

/// Rewrite just the `model` field in a JSON body, used when a route's
/// `remap` table changes the model id. Falls back to the original body
/// untouched if the body isn't an object.
///
/// This does lose serde_json's default key ordering. We accept that on
/// the remap path; on the (overwhelmingly common) no-remap path the
/// body is forwarded byte-for-byte without ever touching this function.
fn rewrite_model(body: &Bytes, new_model: &str) -> Result<Bytes, ForwardError> {
    let mut value: serde_json::Value = serde_json::from_slice(body).map_err(ForwardError::BodyParse)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "model".to_string(),
            serde_json::Value::String(new_model.to_string()),
        );
    }
    let bytes = serde_json::to_vec(&value).map_err(ForwardError::BodySerialize)?;
    Ok(Bytes::from(bytes))
}

/// Wrap the upstream `reqwest::Response` in an `axum::Response` whose
/// body is a streaming wrapper. Each chunk is forwarded to the client
/// as it arrives — no buffering. This is the whole reason we picked
/// Rust + Tokio for this layer.
fn relay_response(upstream: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);

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

    #[error("could not parse request body as JSON for remap: {0}")]
    BodyParse(#[source] serde_json::Error),

    #[error("could not re-serialize JSON body after remap: {0}")]
    BodySerialize(#[source] serde_json::Error),

    #[error("invalid header value while building upstream request: {0}")]
    InvalidHeaderValue(axum::http::header::InvalidHeaderValue),
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Uri;

    #[test]
    fn url_combines_base_and_path_with_query() {
        let uri: Uri = "/v1/messages?beta=true".parse().unwrap();
        assert_eq!(
            build_upstream_url("https://api.anthropic.com", &uri),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
    }

    #[test]
    fn url_handles_trailing_slash_on_base() {
        let uri: Uri = "/v1/messages".parse().unwrap();
        assert_eq!(
            build_upstream_url("https://api.anthropic.com/", &uri),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn url_preserves_base_path_prefix() {
        // DeepSeek case: the Anthropic-compatible API lives under /anthropic.
        let uri: Uri = "/v1/messages".parse().unwrap();
        assert_eq!(
            build_upstream_url("https://api.deepseek.com/anthropic", &uri),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn passthrough_keeps_client_auth_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer sk-ant-oat01-EXAMPLE".parse().unwrap(),
        );
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

        let provider = Provider {
            provider_type: crate::config::ProviderType::AnthropicCompatible,
            url: "https://api.anthropic.com".into(),
            api_key: None,
            passthrough_auth: true,
            models: vec![],
        };

        let out = build_upstream_headers(&headers, &provider).unwrap();
        assert_eq!(
            out.get("authorization").unwrap().to_str().unwrap(),
            "Bearer sk-ant-oat01-EXAMPLE"
        );
        assert!(out.get("anthropic-version").is_some());
        assert!(out.get("x-api-key").is_none());
    }

    #[test]
    fn non_passthrough_substitutes_x_api_key_and_drops_client_auth() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer client-supplied".parse().unwrap());
        headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

        let provider = Provider {
            provider_type: crate::config::ProviderType::AnthropicCompatible,
            url: "https://api.deepseek.com/anthropic".into(),
            api_key: Some("sk-deepseek-xyz".into()),
            passthrough_auth: false,
            models: vec![],
        };

        let out = build_upstream_headers(&headers, &provider).unwrap();
        assert_eq!(
            out.get("x-api-key").unwrap().to_str().unwrap(),
            "sk-deepseek-xyz"
        );
        // Client's authorization must be gone — we don't ship two keys.
        assert!(out.get("authorization").is_none());
        // End-to-end headers untouched.
        assert_eq!(
            out.get("anthropic-version").unwrap().to_str().unwrap(),
            "2023-06-01"
        );
    }

    #[test]
    fn missing_api_key_when_required_errors() {
        let provider = Provider {
            provider_type: crate::config::ProviderType::AnthropicCompatible,
            url: "https://example.com".into(),
            api_key: None,
            passthrough_auth: false,
            models: vec![],
        };
        let err = build_upstream_headers(&HeaderMap::new(), &provider).unwrap_err();
        assert!(matches!(err, ForwardError::MissingApiKey));
    }

    #[test]
    fn hop_by_hop_headers_are_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", "keep-alive".parse().unwrap());
        headers.insert("keep-alive", "timeout=60".parse().unwrap());
        headers.insert("transfer-encoding", "chunked".parse().unwrap());
        headers.insert("host", "127.0.0.1:3300".parse().unwrap());
        headers.insert("content-length", "42".parse().unwrap());
        headers.insert("x-stainless-os", "MacOS".parse().unwrap()); // end-to-end

        let provider = Provider {
            provider_type: crate::config::ProviderType::AnthropicCompatible,
            url: "https://example.com".into(),
            api_key: None,
            passthrough_auth: true,
            models: vec![],
        };

        let out = build_upstream_headers(&headers, &provider).unwrap();
        assert!(out.get("connection").is_none());
        assert!(out.get("keep-alive").is_none());
        assert!(out.get("transfer-encoding").is_none());
        assert!(out.get("host").is_none());
        assert!(out.get("content-length").is_none());
        assert!(out.get("x-stainless-os").is_some(), "end-to-end header must survive");
    }

    #[test]
    fn proxy_disclosing_headers_are_not_added() {
        // The forward path should never inject these. We test by checking
        // the output for absence after a build_upstream_headers call that
        // started without them.
        let provider = Provider {
            provider_type: crate::config::ProviderType::AnthropicCompatible,
            url: "https://example.com".into(),
            api_key: Some("k".into()),
            passthrough_auth: false,
            models: vec![],
        };
        let out = build_upstream_headers(&HeaderMap::new(), &provider).unwrap();
        for tell in ["via", "x-forwarded-for", "x-forwarded-host", "x-real-ip", "forwarded"] {
            assert!(out.get(tell).is_none(), "{tell} must not be added by the proxy");
        }
    }

    #[test]
    fn rewrite_model_replaces_only_top_level_model() {
        let body = Bytes::from(r#"{"model":"old","messages":[],"max_tokens":1}"#);
        let out = rewrite_model(&body, "new").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["model"], "new");
        assert_eq!(parsed["max_tokens"], 1);
    }
}
