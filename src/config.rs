//! Configuration loading for `open-interceptor`.
//!
//! Reads YAML from a file path, expands `${ENV_VAR}` placeholders inside
//! string fields (typically used in `api_key`), and validates that every
//! route references a provider that exists.
//!
//! The structs here are the canonical schema — see `config.yaml.example`
//! at the repo root for a documented example.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level config.
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    pub providers: HashMap<String, Provider>,

    pub routes: Vec<Route>,
}

#[derive(Debug, Deserialize)]
pub struct Provider {
    /// Discriminator: how to talk to this provider.
    #[serde(rename = "type")]
    pub provider_type: ProviderType,

    /// Upstream base URL. For `anthropic_compatible` providers this is the
    /// origin (e.g. `https://api.anthropic.com`). For `openai_compatible`
    /// it points to the chat-completions root.
    pub url: String,

    /// API key, with `${VAR}` expansion already applied at load time.
    /// Optional because `passthrough_auth: true` plus an OAuth client
    /// makes the proxy not own a key at all.
    #[serde(default)]
    pub api_key: Option<String>,

    /// When true, the proxy forwards the client's auth header unchanged
    /// to upstream — used to keep a Pro/Max subscription session alive
    /// instead of substituting with `api_key`. See
    /// `docs/claude-code-headers.md`.
    #[serde(default)]
    pub passthrough_auth: bool,

    /// Static list of model IDs this provider exposes, surfaced via the
    /// `/v1/models` endpoint added in Phase 2. Optional — empty means the
    /// proxy will (eventually) probe the provider's own `/v1/models`.
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    AnthropicCompatible,
    OpenaiCompatible,
    Passthrough,
}

#[derive(Debug, Deserialize)]
pub struct Route {
    /// Glob patterns to match against the incoming `model` field. The
    /// first route whose pattern set contains a match wins.
    pub models: Vec<String>,

    /// Key into `Config.providers`.
    pub provider: String,

    /// Optional model-id rewrites applied before dispatching to the
    /// upstream provider. Useful when the name Claude Code uses differs
    /// from the provider's canonical id.
    #[serde(default)]
    pub remap: HashMap<String, String>,
}

fn default_port() -> u16 {
    3300
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Walk a YAML `Value` tree and expand `${ENV_VAR}` inside every string
/// scalar (mapping values, sequence items, nested structures). Mapping
/// keys are intentionally left alone — they're identifiers, not config.
fn expand_env_in_strings(value: &mut serde_yml::Value) -> Result<(), ConfigError> {
    use serde_yml::Value;
    match value {
        Value::String(s) => {
            let expanded =
                shellexpand::env(s).map_err(|e| ConfigError::EnvExpansion(e.to_string()))?;
            // Avoid allocating when nothing changed.
            if expanded.as_ref() != s.as_str() {
                *s = expanded.into_owned();
            }
        }
        Value::Sequence(items) => {
            for item in items {
                expand_env_in_strings(item)?;
            }
        }
        Value::Mapping(map) => {
            for (_k, v) in map.iter_mut() {
                expand_env_in_strings(v)?;
            }
        }
        // Null, Bool, Number, Tagged — no strings to expand.
        _ => {}
    }
    Ok(())
}

impl Config {
    /// Read and validate a config file from disk.
    ///
    /// 1. Read the file as UTF-8.
    /// 2. Parse as YAML into a generic `Value` tree.
    /// 3. Walk that tree and expand `${ENV_VAR}` placeholders only inside
    ///    string scalars. This deliberately skips comments (already
    ///    stripped by the parser) and YAML keys, so e.g. a comment
    ///    documenting "use `${VAR}` syntax for api keys" doesn't break
    ///    the loader.
    /// 4. Deserialize the (now-expanded) tree into `Config`.
    /// 5. Validate that every `route.provider` references a known
    ///    provider in `providers` and that no route has empty patterns.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        let mut value: serde_yml::Value = serde_yml::from_str(&raw)?;
        expand_env_in_strings(&mut value)?;

        let config: Config = serde_yml::from_value(value)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        for (i, route) in self.routes.iter().enumerate() {
            if !self.providers.contains_key(&route.provider) {
                return Err(ConfigError::UnknownProvider {
                    route_index: i,
                    provider: route.provider.clone(),
                });
            }
            if route.models.is_empty() {
                return Err(ConfigError::EmptyRoutePatterns { route_index: i });
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("env var expansion failed: {0}")]
    EnvExpansion(String),

    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yml::Error),

    #[error("route #{route_index} references unknown provider `{provider}`")]
    UnknownProvider {
        route_index: usize,
        provider: String,
    },

    #[error("route #{route_index} has an empty `models` list")]
    EmptyRoutePatterns { route_index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
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
        let cfg = Config::load(f.path()).unwrap();
        assert_eq!(cfg.port, 3300);
        assert_eq!(cfg.providers.len(), 1);
        let p = &cfg.providers["anthropic"];
        assert_eq!(p.provider_type, ProviderType::AnthropicCompatible);
        assert!(p.passthrough_auth);
        assert!(p.api_key.is_none());
    }

    #[test]
    fn expands_env_vars() {
        // SAFETY: tests run in a single process, but `set_var` is now
        // `unsafe` in Rust 2024. We isolate by using a unique var name.
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
        let cfg = Config::load(f.path()).unwrap();
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
        match Config::load(f.path()) {
            Err(ConfigError::UnknownProvider { provider, .. }) => {
                assert_eq!(provider, "does-not-exist");
            }
            other => panic!("expected UnknownProvider, got {other:?}"),
        }
    }

    #[test]
    fn comments_with_dollar_var_dont_break_loading() {
        // Comments are stripped by the YAML parser, so a `${VAR}` inside
        // a comment must never reach shellexpand.
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
        let cfg = Config::load(f.path()).expect("comment with ${VAR} should be harmless");
        assert!(cfg.providers["p"].passthrough_auth);
    }

    #[test]
    fn unresolved_env_var_in_value_is_an_error() {
        // Conversely, a real ${VAR} reference in an actual config value
        // must fail loudly so the user knows their secret is missing.
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
        match Config::load(f.path()) {
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
        match Config::load(f.path()) {
            Err(ConfigError::EmptyRoutePatterns { .. }) => {}
            other => panic!("expected EmptyRoutePatterns, got {other:?}"),
        }
    }
}
