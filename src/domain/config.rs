//! Domain types for `open-interceptor` configuration.
//!
//! Pure types and validation — no I/O, no YAML parsing, no env var expansion.
//! See `config.yaml.example` at the repo root for a documented example.
//!
//! I/O operations (load, save, env expansion) live in `services::config`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    pub providers: HashMap<String, Provider>,

    pub routes: Vec<Route>,
}

/// Upstream API endpoint configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Multiple API keys for round-robin rotation and automatic failover.
    /// When set alongside `api_key`, both are merged (deduplicated order
    /// preserved). When neither is set, the provider must use
    /// `passthrough_auth` or every request will fail with MissingApiKey.
    #[serde(default)]
    pub api_keys: Option<Vec<String>>,

    /// Key rotation strategy. `round_robin` rotates on every request;
    /// `failover` pins to the first key and only switches on rate-limit
    /// (HTTP 429 or quota-exhausted response).
    #[serde(default)]
    pub key_strategy: Option<KeyStrategy>,

    /// When true, the proxy forwards the client's auth header unchanged
    /// to upstream — used to keep a Pro/Max subscription session alive
    /// instead of substituting with `api_key`.
    #[serde(default)]
    pub passthrough_auth: bool,

    /// Static list of models this provider exposes, surfaced via the
    /// `/v1/models` endpoint. Optional — empty means the proxy will probe
    /// the provider's own `/v1/models` and cache the result (dynamic fetch).
    #[serde(default)]
    pub models: Vec<ModelSpec>,
}

impl Provider {
    pub fn all_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        if let Some(ref k) = self.api_key {
            keys.push(k.clone());
        }
        if let Some(ref ks) = self.api_keys {
            for k in ks {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        keys
    }

    pub fn effective_strategy(&self) -> KeyStrategy {
        self.key_strategy.unwrap_or_default()
    }
}

/// Per-model metadata declared in `config.yaml` under a provider's `models:` list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    /// Total context window in tokens.
    #[serde(default)]
    pub context_window: Option<u32>,
    /// Maximum output tokens the model supports.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    AnthropicCompatible,
    OpenaiCompatible,
    Passthrough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStrategy {
    #[default]
    RoundRobin,
    Failover,
}

/// Route: matches incoming model requests to providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    /// Glob patterns to match against the incoming `model` field. The
    /// first route whose pattern set contains a match wins.
    pub models: Vec<String>,

    /// Key into `Config.providers`.
    pub provider: String,

    /// Optional model-id rewrites applied before dispatching to the
    /// upstream provider.
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
    /// Validate business rules: every route references an existing provider,
    /// and no route has empty model patterns.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        for (i, route) in self.routes.iter().enumerate() {
            if !self.providers.contains_key(&route.provider) {
                return Err(ConfigValidationError::UnknownProvider {
                    route_index: i,
                    provider: route.provider.clone(),
                });
            }
            if route.models.is_empty() {
                return Err(ConfigValidationError::EmptyRoutePatterns { route_index: i });
            }
        }
        Ok(())
    }
}

/// Validation errors for domain config rules (no I/O).
#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("route #{route_index} references unknown provider `{provider}`")]
    UnknownProvider {
        route_index: usize,
        provider: String,
    },

    #[error("route #{route_index} has an empty `models` list")]
    EmptyRoutePatterns { route_index: usize },
}
