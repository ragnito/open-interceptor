//! `/v1/models` endpoint spoofing (Phase 2).
//!
//! Claude Code queries `GET /v1/models` at startup when
//! `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` is set. This module
//! responds with the union of every configured provider's declared model
//! IDs in the Anthropic shape so they appear natively in Claude Code's
//! `/model` picker.
//!
//! For providers that declare a static `models:` list in config.yaml, those
//! IDs are included directly. For providers without a list (e.g. OpenRouter
//! where the user wants the gateway to populate the picker dynamically), the
//! handler can optionally fetch the provider's own `/v1/models` and cache
//! the result with a configurable TTL.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use axum::{Json, extract::State};
use serde::Serialize;
use tokio::time::Instant;

use crate::domain::config::ProviderType;
use crate::domain::router::Router;

/// Top-level Anthropic `/v1/models` shape.
#[derive(Serialize)]
pub struct ModelsResponse {
    pub data: Vec<ModelEntry>,
    #[serde(rename = "first_id")]
    pub first_id: Option<String>,
    #[serde(rename = "last_id")]
    pub last_id: Option<String>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct ModelEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: String,
    pub display_name: String,
    /// Serialized as `max_input_tokens` — that's the field name the real
    /// Anthropic `/v1/models` response uses, and what Claude Code reads to
    /// populate `/context`. We keep the internal Rust name `context_window`
    /// because it matches the YAML key the user writes.
    #[serde(rename = "max_input_tokens", skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Serialized as `max_tokens` for the same reason — matches the
    /// upstream Anthropic schema.
    #[serde(rename = "max_tokens", skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// In-memory cache for dynamically-fetched model lists.
///
/// Keyed by provider key, stores the fetched model IDs and an expiry
/// timestamp. The `RwLock` lets concurrent read requests share the cache
/// without contention; writes happen only on cache miss or expiry.
#[derive(Default)]
struct ModelCache {
    entries: RwLock<HashMap<String, CachedProviderModels>>,
}

struct CachedProviderModels {
    specs: Vec<crate::domain::config::ModelSpec>,
    expires_at: Instant,
}

/// Shared state the handler reads at serve-time.
pub struct ModelsState {
    router: std::sync::Arc<Router>,
    cache: ModelCache,
    /// How long to keep a dynamically-fetched model list before re-fetching.
    /// Defaults to 1 hour. Set to 0 to disable caching.
    cache_ttl: Duration,
}

impl ModelsState {
    pub fn new(router: std::sync::Arc<Router>) -> Self {
        Self {
            router,
            cache: ModelCache::default(),
            cache_ttl: Duration::from_secs(3600),
        }
    }
}

/// Axum handler for `GET /v1/models`.
///
/// 1. Collects static model IDs from all providers that declare them.
/// 2. For providers without a static list, checks the cache; if stale or
///    absent, fetches from the provider's own `/v1/models` (only for
///    `anthropic_compatible` type — OpenAI-compatible providers skip
///    dynamic fetch since they don't speak this endpoint).
/// 3. Returns the deduplicated union in Anthropic shape.
pub async fn handle_models(
    State(state): State<std::sync::Arc<ModelsState>>,
) -> Json<ModelsResponse> {
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<ModelEntry> = Vec::new();

    for (provider_key, provider) in state.router.providers() {
        if !provider.models.is_empty() {
            // Static list from config.
            for model in &provider.models {
                if seen.insert(model.id.clone()) {
                    entries.push(ModelEntry {
                        entry_type: "model".into(),
                        id: model.id.clone(),
                        display_name: model.id.clone(),
                        context_window: model.context_window,
                        max_output_tokens: model.max_output_tokens,
                    });
                }
            }
        } else if matches!(provider.provider_type, ProviderType::AnthropicCompatible) {
            // Try dynamic fetch against the provider's own /v1/models.
            let specs = state.fetch_or_cache(provider_key, &provider.url).await;
            for spec in specs {
                if seen.insert(spec.id.clone()) {
                    entries.push(ModelEntry {
                        entry_type: "model".into(),
                        display_name: spec.id.clone(),
                        id: spec.id,
                        context_window: spec.context_window,
                        max_output_tokens: spec.max_output_tokens,
                    });
                }
            }
        }
    }

    let _total = entries.len();
    Json(ModelsResponse {
        data: entries,
        first_id: None,
        last_id: None,
        has_more: false,
    })
}

impl ModelsState {
    /// Return model specs for a provider, using cache if available.
    async fn fetch_or_cache(&self, key: &str, url: &str) -> Vec<crate::domain::config::ModelSpec> {
        // Check cache first.
        {
            let cache = self.cache.entries.read().expect("model cache lock");
            if let Some(entry) = cache.get(key)
                && (self.cache_ttl == Duration::ZERO || entry.expires_at > Instant::now())
            {
                return entry.specs.clone();
            }
        }

        // Cache miss or stale — fetch.
        let specs = fetch_provider_models(url).await;

        let expires_at = Instant::now() + self.cache_ttl;
        self.cache
            .entries
            .write()
            .expect("model cache lock")
            .insert(
                key.to_string(),
                CachedProviderModels {
                    specs: specs.clone(),
                    expires_at,
                },
            );

        specs
    }
}

/// Fetch `/v1/models` from an Anthropic-compatible provider and extract
/// model specs. Returns an empty Vec on any error — graceful degradation.
async fn fetch_provider_models(base_url: &str) -> Vec<crate::domain::config::ModelSpec> {
    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let url = format!("{base}/v1/models");

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .build()
        .ok();

    let client = match client {
        Some(c) => c,
        None => return vec![],
    };

    let resp = match client
        .get(&url)
        .header("anthropic-version", "2023-06-01")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%url, "failed to fetch provider /v1/models: {e}");
            return vec![];
        }
    };

    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(%url, %status, "provider /v1/models returned non-200");
        return vec![];
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(%url, "failed to parse provider /v1/models: {e}");
            return vec![];
        }
    };

    // Anthropic shape: { data: [{ id, type, max_input_tokens?, max_tokens?, ... }] }
    // OpenAI shape:   { data: [{ id, object, ... }] }
    // We accept the legacy `context_window` / `max_output_tokens` names too
    // for upstreams that use them.
    if let Some(data) = body["data"].as_array() {
        data.iter()
            .filter_map(|entry| {
                let id = entry["id"].as_str()?.to_string();
                let context_window = entry["max_input_tokens"]
                    .as_u64()
                    .or_else(|| entry["context_window"].as_u64())
                    .map(|v| v as u32);
                let max_output_tokens = entry["max_tokens"]
                    .as_u64()
                    .or_else(|| entry["max_output_tokens"].as_u64())
                    .map(|v| v as u32);
                Some(crate::domain::config::ModelSpec {
                    id,
                    context_window,
                    max_output_tokens,
                })
            })
            .collect()
    } else {
        vec![]
    }
}
