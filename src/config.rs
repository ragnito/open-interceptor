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

impl Config {
    /// Read and validate a config file from disk.
    ///
    /// 1. Read the file as UTF-8.
    /// 2. Expand `${ENV_VAR}` placeholders via `shellexpand::env`.
    /// 3. Parse as YAML.
    /// 4. Validate that every `route.provider` references a known
    ///    provider in `providers`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        let expanded =
            shellexpand::env(&raw).map_err(|e| ConfigError::EnvExpansion(e.to_string()))?;

        let config: Config = serde_yml::from_str(&expanded)?;
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
