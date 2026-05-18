//! Integration tests for `domain::config`.

use std::collections::HashMap;

use open_interceptor::domain::config::{
    Config, ConfigValidationError, Provider, ProviderType, Route,
};

fn minimal_config() -> Config {
    Config {
        port: 3300,
        log_level: "info".to_string(),
        providers: {
            let mut m = HashMap::new();
            m.insert(
                "anthropic".to_string(),
                Provider {
                    provider_type: ProviderType::AnthropicCompatible,
                    url: "https://api.anthropic.com".to_string(),
                    api_key: None,
                    passthrough_auth: true,
                    models: vec![],
                },
            );
            m
        },
        routes: vec![Route {
            models: vec!["claude-*".to_string()],
            provider: "anthropic".to_string(),
            remap: HashMap::new(),
        }],
    }
}

#[test]
fn valid_config_passes_validation() {
    let cfg = minimal_config();
    assert!(cfg.validate().is_ok());
}

#[test]
fn unknown_provider_fails_validation() {
    let cfg = Config {
        routes: vec![Route {
            models: vec!["*".to_string()],
            provider: "nonexistent".to_string(),
            remap: HashMap::new(),
        }],
        ..minimal_config()
    };
    match cfg.validate() {
        Err(ConfigValidationError::UnknownProvider { provider, .. }) => {
            assert_eq!(provider, "nonexistent");
        }
        other => panic!("expected UnknownProvider, got {other:?}"),
    }
}

#[test]
fn empty_route_patterns_fails_validation() {
    let cfg = Config {
        routes: vec![Route {
            models: vec![],
            provider: "anthropic".to_string(),
            remap: HashMap::new(),
        }],
        ..minimal_config()
    };
    match cfg.validate() {
        Err(ConfigValidationError::EmptyRoutePatterns { .. }) => {}
        other => panic!("expected EmptyRoutePatterns, got {other:?}"),
    }
}
