//! Curated provider catalog offered by the first-run wizard.
//!
//! The entries mirror the documented setups in `config.yaml.example`, so the
//! generated config is one the maintainer already vouches for. Model ids are
//! deliberately copied from that file rather than invented: a wrong id
//! produces a config that only fails later, at request time.
//!
//! Anything not covered here is still reachable — the wizard prints the config
//! path so the file can be edited by hand.

use std::collections::HashMap;

use crate::domain::config::{ModelSpec, Provider, ProviderType, Route};

/// A single text input the wizard asks for on behalf of an entry.
pub struct FieldSpec {
    /// Stable id used to look the answer up in the collected values.
    pub id: &'static str,
    pub label: &'static str,
    /// Shown greyed out while the field is empty.
    pub placeholder: &'static str,
    /// API keys are rendered as bullets so they don't linger on screen.
    pub masked: bool,
}

/// Which provider set an entry expands into. The wizard only ever shows the
/// catalog; this discriminant drives config generation in [`build`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Anthropic with the client's own auth forwarded (Pro/Max subscription).
    Anthropic,
    /// OpenCode Go — one subscription, two providers (native Anthropic models
    /// and OpenAI-format models behind the translation layer).
    OpenCodeGo,
    /// OpenAI proper.
    OpenAi,
    /// Any other OpenAI-compatible endpoint (LM Studio, Ollama, a gateway…).
    CustomOpenAi,
}

pub struct CatalogEntry {
    pub kind: EntryKind,
    pub label: &'static str,
    pub blurb: &'static str,
    /// Pre-ticked in the selection list.
    pub default_selected: bool,
    pub fields: &'static [FieldSpec],
}

const KEY_FIELD_OPENCODE: FieldSpec = FieldSpec {
    id: "opencode_key",
    label: "OpenCode Go API key",
    placeholder: "sk-… (or ${OPENCODE_GO_API_KEY})",
    masked: true,
};

const KEY_FIELD_OPENAI: FieldSpec = FieldSpec {
    id: "openai_key",
    label: "OpenAI API key",
    placeholder: "sk-… (or ${OPENAI_API_KEY})",
    masked: true,
};

const CUSTOM_FIELDS: [FieldSpec; 3] = [
    FieldSpec {
        id: "custom_url",
        label: "Base URL (must include /v1)",
        placeholder: "http://localhost:1234/v1",
        masked: false,
    },
    FieldSpec {
        id: "custom_models",
        label: "Model ids (comma separated)",
        placeholder: "my-model, another-model",
        masked: false,
    },
    FieldSpec {
        id: "custom_key",
        label: "API key (blank if not required)",
        placeholder: "leave empty for local servers",
        masked: true,
    },
];

/// Everything the wizard can offer, in display order.
pub fn catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            kind: EntryKind::Anthropic,
            label: "Anthropic (Claude Pro/Max subscription)",
            blurb: "Forwards Claude Code's own auth — no API key needed.",
            default_selected: true,
            fields: &[],
        },
        CatalogEntry {
            kind: EntryKind::OpenCodeGo,
            label: "OpenCode Go",
            blurb: "One subscription, 12 open models (MiniMax, GLM, Kimi, DeepSeek, Qwen…).",
            default_selected: false,
            fields: std::slice::from_ref(&KEY_FIELD_OPENCODE),
        },
        CatalogEntry {
            kind: EntryKind::OpenAi,
            label: "OpenAI",
            blurb: "gpt-5, gpt-4o, o3-mini via the translation layer.",
            default_selected: false,
            fields: std::slice::from_ref(&KEY_FIELD_OPENAI),
        },
        CatalogEntry {
            kind: EntryKind::CustomOpenAi,
            label: "Custom OpenAI-compatible endpoint",
            blurb: "Any /v1/chat/completions server: LM Studio, Ollama, a gateway…",
            default_selected: false,
            fields: &CUSTOM_FIELDS,
        },
    ]
}

fn models(specs: &[(&str, u32)]) -> Vec<ModelSpec> {
    specs
        .iter()
        .map(|(id, ctx)| ModelSpec {
            id: (*id).to_string(),
            context_window: Some(*ctx),
            max_output_tokens: None,
        })
        .collect()
}

fn route(patterns: &[&str], provider: &str) -> Route {
    Route {
        models: patterns.iter().map(|p| (*p).to_string()).collect(),
        provider: provider.to_string(),
        remap: HashMap::new(),
    }
}

/// Expand one selected entry into the providers and routes it contributes.
///
/// `values` holds the answers to that entry's [`CatalogEntry::fields`], keyed
/// by [`FieldSpec::id`]. Routes are returned in the order they should be
/// evaluated; the caller appends the catch-all last.
pub fn build(
    kind: EntryKind,
    values: &HashMap<String, String>,
) -> (Vec<(String, Provider)>, Vec<Route>) {
    let get = |id: &str| values.get(id).map(|s| s.trim()).unwrap_or_default();
    // An empty answer must not become `api_key: ""` — that would look
    // configured while failing every request.
    let key_of = |id: &str| {
        let v = get(id);
        if v.is_empty() {
            None
        } else {
            Some(v.to_string())
        }
    };

    match kind {
        EntryKind::Anthropic => (
            vec![(
                "anthropic".to_string(),
                Provider {
                    provider_type: ProviderType::AnthropicCompatible,
                    url: "https://api.anthropic.com".to_string(),
                    api_key: None,
                    api_keys: None,
                    key_strategy: None,
                    passthrough_auth: true,
                    models: models(&[
                        ("claude-opus-4-7", 200_000),
                        ("claude-sonnet-4-6", 200_000),
                        ("claude-haiku-4-5", 200_000),
                    ]),
                },
            )],
            vec![route(&["claude-*"], "anthropic")],
        ),

        EntryKind::OpenCodeGo => {
            let key = key_of("opencode_key");
            (
                vec![
                    (
                        "opencode-go-anthropic".to_string(),
                        Provider {
                            provider_type: ProviderType::AnthropicCompatible,
                            url: "https://opencode.ai/zen/go".to_string(),
                            api_key: key.clone(),
                            api_keys: None,
                            key_strategy: None,
                            passthrough_auth: false,
                            models: models(&[
                                ("minimax-m2.7", 1_000_000),
                                ("minimax-m2.5", 1_000_000),
                            ]),
                        },
                    ),
                    (
                        "opencode-go-openai".to_string(),
                        Provider {
                            provider_type: ProviderType::OpenaiCompatible,
                            url: "https://opencode.ai/zen/go/v1".to_string(),
                            api_key: key,
                            api_keys: None,
                            key_strategy: None,
                            passthrough_auth: false,
                            models: models(&[
                                ("glm-5.1", 128_000),
                                ("glm-5", 128_000),
                                ("kimi-k2.6", 128_000),
                                ("kimi-k2.5", 128_000),
                                ("deepseek-v4-pro", 128_000),
                                ("deepseek-v4-flash", 128_000),
                                ("mimo-v2.5", 32_768),
                                ("mimo-v2.5-pro", 32_768),
                                ("qwen3.6-plus", 128_000),
                                ("qwen3.5-plus", 128_000),
                            ]),
                        },
                    ),
                ],
                vec![
                    route(&["minimax-*"], "opencode-go-anthropic"),
                    route(
                        &["glm-*", "kimi-*", "deepseek-v4-*", "mimo-*", "qwen3*"],
                        "opencode-go-openai",
                    ),
                ],
            )
        }

        EntryKind::OpenAi => (
            vec![(
                "openai".to_string(),
                Provider {
                    provider_type: ProviderType::OpenaiCompatible,
                    url: "https://api.openai.com/v1".to_string(),
                    api_key: key_of("openai_key"),
                    api_keys: None,
                    key_strategy: None,
                    passthrough_auth: false,
                    models: models(&[
                        ("gpt-5", 400_000),
                        ("gpt-4o", 128_000),
                        ("o3-mini", 200_000),
                    ]),
                },
            )],
            vec![route(&["gpt-*", "o1-*", "o3-*", "o4-*"], "openai")],
        ),

        EntryKind::CustomOpenAi => {
            let url = get("custom_url");
            let ids: Vec<String> = get("custom_models")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            // Route on the exact ids the user listed. Without them we cannot
            // build a meaningful route, so the entry contributes nothing
            // rather than a catch-all that would shadow every other provider.
            let patterns: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
            let routes = if patterns.is_empty() {
                vec![]
            } else {
                vec![route(&patterns, "custom")]
            };

            (
                vec![(
                    "custom".to_string(),
                    Provider {
                        provider_type: ProviderType::OpenaiCompatible,
                        url: url.to_string(),
                        api_key: key_of("custom_key"),
                        api_keys: None,
                        key_strategy: None,
                        passthrough_auth: false,
                        models: ids
                            .iter()
                            .map(|id| ModelSpec {
                                id: id.clone(),
                                context_window: None,
                                max_output_tokens: None,
                            })
                            .collect(),
                    },
                )],
                routes,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn anthropic_uses_passthrough_and_needs_no_key() {
        let (providers, routes) = build(EntryKind::Anthropic, &HashMap::new());
        let (_, p) = &providers[0];
        assert!(p.passthrough_auth);
        assert!(p.api_key.is_none());
        assert_eq!(routes[0].provider, "anthropic");
    }

    #[test]
    fn opencode_shares_one_key_across_both_providers() {
        let (providers, routes) =
            build(EntryKind::OpenCodeGo, &vals(&[("opencode_key", "sk-abc")]));
        assert_eq!(providers.len(), 2);
        for (_, p) in &providers {
            assert_eq!(p.api_key.as_deref(), Some("sk-abc"));
        }
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn blank_key_stays_none_rather_than_empty_string() {
        let (providers, _) = build(EntryKind::OpenAi, &vals(&[("openai_key", "   ")]));
        assert!(providers[0].1.api_key.is_none());
    }

    #[test]
    fn custom_entry_routes_only_the_ids_given() {
        let (providers, routes) = build(
            EntryKind::CustomOpenAi,
            &vals(&[
                ("custom_url", "http://localhost:1234/v1"),
                ("custom_models", " local-a , local-b "),
            ]),
        );
        assert_eq!(providers[0].1.url, "http://localhost:1234/v1");
        assert_eq!(routes[0].models, vec!["local-a", "local-b"]);
    }

    #[test]
    fn custom_entry_without_models_contributes_no_route() {
        let (_, routes) = build(
            EntryKind::CustomOpenAi,
            &vals(&[("custom_url", "http://localhost:1234/v1")]),
        );
        assert!(
            routes.is_empty(),
            "must not emit a route that matches nothing useful"
        );
    }

    #[test]
    fn every_catalog_field_id_is_unique() {
        let mut seen = Vec::new();
        for entry in catalog() {
            for f in entry.fields {
                assert!(!seen.contains(&f.id), "duplicate field id: {}", f.id);
                seen.push(f.id);
            }
        }
    }
}
