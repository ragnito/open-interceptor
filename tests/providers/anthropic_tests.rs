//! Integration tests for `open_interceptor::providers::anthropic`.

use axum::http::{HeaderMap, HeaderValue};
use open_interceptor::providers::anthropic::{
    build_upstream_headers, build_upstream_url, sanitize_body, ForwardError,
};
use open_interceptor::domain::config::{Provider, ProviderType};
use std::collections::HashMap;

#[test]
fn url_combines_base_and_path_with_query() {
    let uri: axum::http::Uri = "/v1/messages?beta=true".parse().unwrap();
    assert_eq!(
        build_upstream_url("https://api.anthropic.com", &uri),
        "https://api.anthropic.com/v1/messages?beta=true"
    );
}

#[test]
fn url_handles_trailing_slash_on_base() {
    let uri: axum::http::Uri = "/v1/messages".parse().unwrap();
    assert_eq!(
        build_upstream_url("https://api.anthropic.com/", &uri),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn url_preserves_base_path_prefix() {
    // DeepSeek case: the Anthropic-compatible API lives under /anthropic.
    let uri: axum::http::Uri = "/v1/messages".parse().unwrap();
    assert_eq!(
        build_upstream_url("https://api.deepseek.com/anthropic", &uri),
        "https://api.deepseek.com/anthropic/v1/messages"
    );
}

#[test]
fn url_strips_trailing_v1_to_avoid_duplicate() {
    let uri: axum::http::Uri = "/v1/messages".parse().unwrap();
    assert_eq!(
        build_upstream_url("https://api.anthropic.com/v1", &uri),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        build_upstream_url("https://api.anthropic.com/v1/", &uri),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn passthrough_keeps_client_auth_header() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer sk-ant-oat01-EXAMPLE"),
    );
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static("2023-06-01"),
    );

    let provider = Provider {
        provider_type: ProviderType::AnthropicCompatible,
        url: "https://api.anthropic.com".into(),
        api_key: None,
        passthrough_auth: true,
        models: vec![],
    };

    let out = build_upstream_headers(&headers, &provider, None).unwrap();
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
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer client-supplied"),
    );
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static("2023-06-01"),
    );

    let provider = Provider {
        provider_type: ProviderType::AnthropicCompatible,
        url: "https://api.deepseek.com/anthropic".into(),
        api_key: Some("sk-deepseek-xyz".into()),
        passthrough_auth: false,
        models: vec![],
    };

    let out = build_upstream_headers(&headers, &provider, Some("sk-deepseek-xyz")).unwrap();
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
        provider_type: ProviderType::AnthropicCompatible,
        url: "https://example.com".into(),
        api_key: None,
        passthrough_auth: false,
        models: vec![],
    };
    let err = build_upstream_headers(&HeaderMap::new(), &provider, None).unwrap_err();
    assert!(matches!(err, ForwardError::MissingApiKey));
}

#[test]
fn hop_by_hop_headers_are_dropped() {
    let mut headers = HeaderMap::new();
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert("keep-alive", HeaderValue::from_static("timeout=60"));
    headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    headers.insert("host", HeaderValue::from_static("127.0.0.1:3300"));
    headers.insert("content-length", HeaderValue::from_static("42"));
    headers.insert("x-stainless-os", HeaderValue::from_static("MacOS")); // end-to-end

    let provider = Provider {
        provider_type: ProviderType::AnthropicCompatible,
        url: "https://example.com".into(),
        api_key: None,
        passthrough_auth: true,
        models: vec![],
    };

    let out = build_upstream_headers(&headers, &provider, None).unwrap();
    assert!(out.get("connection").is_none());
    assert!(out.get("keep-alive").is_none());
    assert!(out.get("transfer-encoding").is_none());
    assert!(out.get("host").is_none());
    assert!(out.get("content-length").is_none());
    assert!(
        out.get("x-stainless-os").is_some(),
        "end-to-end header must survive"
    );
}

#[test]
fn proxy_disclosing_headers_are_not_added() {
    // The forward path should never inject these. We test by checking
    // the output for absence after a build_upstream_headers call that
    // started without them.
    let provider = Provider {
        provider_type: ProviderType::AnthropicCompatible,
        url: "https://example.com".into(),
        api_key: Some("k".into()),
        passthrough_auth: false,
        models: vec![],
    };
    let out = build_upstream_headers(&HeaderMap::new(), &provider, Some("k")).unwrap();
    for tell in [
        "via",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-real-ip",
        "forwarded",
    ] {
        assert!(
            out.get(tell).is_none(),
            "{tell} must not be added by the proxy"
        );
    }
}

#[test]
fn rewrite_model_replaces_only_top_level_model() {
    let body = axum::body::Bytes::from(r#"{"model":"old","messages":[],"max_tokens":1}"#);
    let out = sanitize_body(&body, "new", "old").unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["model"], "new");
    assert_eq!(parsed["max_tokens"], 1);
}

#[test]
fn sanitize_strips_thinking_blocks_from_messages() {
    let body = axum::body::Bytes::from(r#"{
        "model": "claude-sonnet-4-6",
        "max_tokens": 100,
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "let me think", "signature": "fake_sig"},
                {"type": "text", "text": "Hello!"}
            ]},
            {"role": "user", "content": "more"}
        ]
    }"#);
    let out = sanitize_body(&body, "claude-sonnet-4-6", "claude-sonnet-4-6").unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let assistant_content = parsed["messages"][1]["content"].as_array().unwrap();
    assert_eq!(assistant_content.len(), 1);
    assert_eq!(assistant_content[0]["type"], "text");
}

#[test]
fn sanitize_also_remaps_model() {
    let body = axum::body::Bytes::from(r#"{"model":"old","max_tokens":1,"messages":[]}"#);
    let out = sanitize_body(&body, "new", "old").unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["model"], "new");
}
