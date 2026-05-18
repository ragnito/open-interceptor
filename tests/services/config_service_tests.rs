//! Integration tests for the config service.

use open_interceptor::services::config::{ConfigError, ConfigService};

fn write_tmp(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    use std::io::Write;
    f.write_all(content.as_bytes()).unwrap();
    f
}

#[test]
fn loads_minimal_valid_config() {
    let yaml = r#"
port: 3300
providers:
  anthropic:
    type: anthropic_compatible
    url: https://api.anthropic.com
    passthrough_auth: true
routes:
  - models: ["claude-*"]
    provider: anthropic
"#;
    let f = write_tmp(yaml);
    let cfg = ConfigService::load(f.path()).unwrap();
    assert_eq!(cfg.port, 3300);
    assert_eq!(cfg.providers.len(), 1);
    let p = &cfg.providers["anthropic"];
    use open_interceptor::domain::config::ProviderType;
    assert_eq!(p.provider_type, ProviderType::AnthropicCompatible);
    assert!(p.passthrough_auth);
    assert!(p.api_key.is_none());
}

#[test]
fn expands_env_vars() {
    unsafe {
        std::env::set_var("OPEN_INTERCEPTOR_TEST_KEY", "sk-abc123");
    }
    let yaml = r#"
providers:
  p:
    type: anthropic_compatible
    url: https://example.com
    api_key: ${OPEN_INTERCEPTOR_TEST_KEY}
routes:
  - models: ["*"]
    provider: p
"#;
    let f = write_tmp(yaml);
    let cfg = ConfigService::load(f.path()).unwrap();
    assert_eq!(cfg.providers["p"].api_key.as_deref(), Some("sk-abc123"));
}

#[test]
fn rejects_route_with_unknown_provider() {
    let yaml = r#"
providers:
  a:
    type: anthropic_compatible
    url: https://example.com
routes:
  - models: ["*"]
    provider: does-not-exist
"#;
    let f = write_tmp(yaml);
    match ConfigService::load(f.path()) {
        Err(ConfigError::Validation(
            open_interceptor::domain::config::ConfigValidationError::UnknownProvider { provider, .. }
        )) => {
            assert_eq!(provider, "does-not-exist");
        }
        other => panic!("expected UnknownProvider, got {other:?}"),
    }
}

#[test]
fn comments_with_dollar_var_dont_break_loading() {
    let yaml = r#"
# Environment variables in api_key fields are expanded with shell syntax: ${VAR}
providers:
  p:
    type: anthropic_compatible
    url: https://example.com
    passthrough_auth: true
routes:
  - models: ["*"]
    provider: p
"#;
    let f = write_tmp(yaml);
    let cfg = ConfigService::load(f.path()).expect("comment with ${VAR} should be harmless");
    assert!(cfg.providers["p"].passthrough_auth);
}

#[test]
fn unresolved_env_var_in_value_is_an_error() {
    let yaml = r#"
providers:
  p:
    type: anthropic_compatible
    url: https://example.com
    api_key: ${OPEN_INTERCEPTOR_DEFINITELY_UNSET_VAR_XYZ}
routes:
  - models: ["*"]
    provider: p
"#;
    let f = write_tmp(yaml);
    match ConfigService::load(f.path()) {
        Err(ConfigError::EnvExpansion(_)) => {}
        other => panic!("expected EnvExpansion error, got {other:?}"),
    }
}

#[test]
fn rejects_empty_route_patterns() {
    let yaml = r#"
providers:
  a:
    type: anthropic_compatible
    url: https://example.com
routes:
  - models: []
    provider: a
"#;
    let f = write_tmp(yaml);
    match ConfigService::load(f.path()) {
        Err(ConfigError::Validation(
            open_interceptor::domain::config::ConfigValidationError::EmptyRoutePatterns { .. }
        )) => {}
        other => panic!("expected EmptyRoutePatterns, got {other:?}"),
    }
}

#[test]
fn loads_models_with_context_window() {
    let yaml = r#"
providers:
  p:
    type: openai_compatible
    url: https://example.com
    api_key: sk-test
    models:
      - id: fast-model
        context_window: 128000
        max_output_tokens: 8192
      - id: big-model
        context_window: 1000000
      - id: minimal-model
routes:
  - models: ["*"]
    provider: p
"#;
    let f = write_tmp(yaml);
    let cfg = ConfigService::load(f.path()).unwrap();
    let models = &cfg.providers["p"].models;
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "fast-model");
    assert_eq!(models[0].context_window, Some(128000));
    assert_eq!(models[0].max_output_tokens, Some(8192));
    assert_eq!(models[1].id, "big-model");
    assert_eq!(models[1].context_window, Some(1000000));
    assert_eq!(models[1].max_output_tokens, None);
    assert_eq!(models[2].id, "minimal-model");
    assert_eq!(models[2].context_window, None);
    assert_eq!(models[2].max_output_tokens, None);
}

#[test]
fn save_and_reload_roundtrip() {
    let yaml = r#"
port: 3300
log_level: debug
providers:
  anthropic:
    type: anthropic_compatible
    url: https://api.anthropic.com
    passthrough_auth: true
    models:
      - id: claude-opus-4-7
        context_window: 200000
routes:
  - models: ["claude-*"]
    provider: anthropic
"#;
    let f = write_tmp(yaml);
    let cfg = ConfigService::load(f.path()).unwrap();
    let out = tempfile::NamedTempFile::new().unwrap();
    ConfigService::save(&cfg, out.path()).unwrap();
    let cfg2 = ConfigService::load(out.path()).unwrap();
    assert_eq!(cfg, cfg2);
}
