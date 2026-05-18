//! Integration tests for `domain::router`.

use std::collections::HashMap;

use open_interceptor::domain::config::{Config, Provider, ProviderType};
use open_interceptor::domain::router::{Router, RouterError};

fn make_provider(r#type: ProviderType, url: &str, passthrough: bool) -> Provider {
    Provider {
        provider_type: r#type,
        url: url.to_string(),
        api_key: None,
        passthrough_auth: passthrough,
        models: vec![],
    }
}

fn make_config(yaml: &str) -> Config {
    // Parse directly from YAML string — no file I/O.
    serde_yml::from_str(yaml).expect("yaml parse")
}

fn load(yaml: &str) -> Router {
    let cfg = make_config(yaml);
    Router::build(cfg).expect("router build")
}

#[test]
fn matches_claude_models_to_anthropic_provider() {
    let r = load(
        r#"
port: 3300
log_level: info
providers:
  anthropic:
    type: anthropic_compatible
    url: https://api.anthropic.com
    passthrough_auth: true
routes:
  - models: ["claude-*"]
    provider: anthropic
"#,
    );
    let res = r.resolve("claude-opus-4-7").unwrap();
    assert_eq!(res.provider_name, "anthropic");
    assert_eq!(res.effective_model, "claude-opus-4-7");
}

#[test]
fn first_matching_route_wins() {
    let r = load(
        r#"
port: 3300
log_level: info
providers:
  a:
    type: anthropic_compatible
    url: https://a.example
    passthrough_auth: true
  b:
    type: anthropic_compatible
    url: https://b.example
    passthrough_auth: true
routes:
  - models: ["claude-opus-*"]
    provider: a
  - models: ["claude-*"]
    provider: b
"#,
    );
    assert_eq!(r.resolve("claude-opus-4-7").unwrap().provider_name, "a");
    assert_eq!(r.resolve("claude-haiku-4-5").unwrap().provider_name, "b");
}

#[test]
fn remap_rewrites_effective_model() {
    let r = load(
        r#"
port: 3300
log_level: info
providers:
  o:
    type: openai_compatible
    url: https://api.openai.com/v1
    api_key: dummy
routes:
  - models: ["gpt-*"]
    provider: o
    remap:
      gpt-5: gpt-5-preview
"#,
    );
    let res = r.resolve("gpt-5").unwrap();
    assert_eq!(res.effective_model, "gpt-5-preview");
    let res = r.resolve("gpt-4o").unwrap();
    assert_eq!(res.effective_model, "gpt-4o");
}

#[test]
fn unmatched_model_returns_none() {
    let r = load(
        r#"
port: 3300
log_level: info
providers:
  a:
    type: anthropic_compatible
    url: https://a.example
    passthrough_auth: true
routes:
  - models: ["claude-*"]
    provider: a
"#,
    );
    assert!(r.resolve("gpt-4o").is_none());
}

#[test]
fn wildcard_catchall_matches_anything() {
    let r = load(
        r#"
port: 3300
log_level: info
providers:
  fallback:
    type: anthropic_compatible
    url: https://fallback.example
    passthrough_auth: true
routes:
  - models: ["*"]
    provider: fallback
"#,
    );
    assert_eq!(
        r.resolve("anything-goes-here").unwrap().provider_name,
        "fallback"
    );
}

#[test]
fn invalid_glob_rejected_at_build_time() {
    let yaml = r#"
port: 3300
log_level: info
providers:
  p:
    type: anthropic_compatible
    url: https://p.example
    passthrough_auth: true
routes:
  - models: ["["]
    provider: p
"#;
    let cfg = make_config(yaml);
    match Router::build(cfg) {
        Err(RouterError::InvalidGlob { route_index, .. }) => {
            assert_eq!(route_index, 0);
        }
        other => panic!("expected InvalidGlob, got {other:?}"),
    }
}
