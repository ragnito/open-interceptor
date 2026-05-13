//! Model → provider routing.
//!
//! Given the `model` field from an incoming request, the router walks the
//! configured routes top-to-bottom and returns the first one whose glob
//! patterns match. Glob compilation happens once at startup via
//! `globset::GlobSet` — the hot path is a single bitset check per route,
//! same crate used by ripgrep.

use std::collections::HashMap;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::{Config, Provider};

/// Router owns the parsed `Config` and the pre-compiled glob matchers.
/// Designed to live behind an `Arc` and be shared across request handlers.
#[derive(Debug)]
pub struct Router {
    port: u16,
    routes: Vec<CompiledRoute>,
    providers: HashMap<String, Provider>,
}

#[derive(Debug)]
struct CompiledRoute {
    globs: GlobSet,
    /// Raw patterns kept around for diagnostics / logging only.
    patterns: Vec<String>,
    provider_name: String,
    remap: HashMap<String, String>,
}

/// Outcome of a successful route resolution. `effective_model` is the
/// model id to forward upstream — same as the requested model unless the
/// route defines an explicit `remap`.
#[derive(Debug)]
pub struct Resolution<'a> {
    pub provider: &'a Provider,
    pub provider_name: &'a str,
    pub effective_model: String,
}

impl Router {
    /// Take a validated `Config` and produce a `Router` with all glob
    /// matchers pre-compiled. Returns an error if any pattern is invalid.
    pub fn build(config: Config) -> Result<Self, RouterError> {
        let mut routes = Vec::with_capacity(config.routes.len());
        for (i, route) in config.routes.into_iter().enumerate() {
            let mut builder = GlobSetBuilder::new();
            for pattern in &route.models {
                let glob = Glob::new(pattern).map_err(|source| RouterError::InvalidGlob {
                    route_index: i,
                    pattern: pattern.clone(),
                    source,
                })?;
                builder.add(glob);
            }
            let globs = builder.build().map_err(|source| RouterError::InvalidGlob {
                route_index: i,
                pattern: route.models.join(", "),
                source,
            })?;
            routes.push(CompiledRoute {
                globs,
                patterns: route.models,
                provider_name: route.provider,
                remap: route.remap,
            });
        }
        Ok(Self {
            port: config.port,
            routes,
            providers: config.providers,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Resolve a model id to its target provider. First route whose glob
    /// set matches wins. Returns `None` only if no route matches and
    /// there's no catch-all — `Config::load` validation does not require
    /// a catch-all, that's the operator's choice.
    pub fn resolve(&self, model: &str) -> Option<Resolution<'_>> {
        for route in &self.routes {
            if route.globs.is_match(model) {
                let provider = self.providers.get(&route.provider_name)?;
                let effective_model = route
                    .remap
                    .get(model)
                    .cloned()
                    .unwrap_or_else(|| model.to_string());
                return Some(Resolution {
                    provider,
                    provider_name: route.provider_name.as_str(),
                    effective_model,
                });
            }
        }
        None
    }

    /// Used by the `/v1/models` endpoint in Phase 2 — iterate all
    /// providers and their declared models.
    pub fn providers(&self) -> impl Iterator<Item = (&str, &Provider)> {
        self.providers.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Debug helper: list of compiled routes, for startup logging.
    pub fn route_summaries(&self) -> Vec<String> {
        self.routes
            .iter()
            .map(|r| format!("{:?} → {}", r.patterns, r.provider_name))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("route #{route_index} has an invalid glob pattern `{pattern}`: {source}")]
    InvalidGlob {
        route_index: usize,
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn load(yaml: &str) -> Router {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(yaml.as_bytes()).unwrap();
        let cfg = Config::load(f.path()).expect("config parse");
        Router::build(cfg).expect("router build")
    }

    #[test]
    fn matches_claude_models_to_anthropic_provider() {
        let r = load(
            r#"
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
        assert_eq!(r.resolve("anything-goes-here").unwrap().provider_name, "fallback");
    }

    #[test]
    fn invalid_glob_rejected_at_build_time() {
        let yaml = r#"
providers:
  p:
    type: anthropic_compatible
    url: https://p.example
    passthrough_auth: true
routes:
  - models: ["["]
    provider: p
"#;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(yaml.as_bytes()).unwrap();
        let cfg = Config::load(f.path()).unwrap();
        match Router::build(cfg) {
            Err(RouterError::InvalidGlob { route_index, .. }) => {
                assert_eq!(route_index, 0);
            }
            other => panic!("expected InvalidGlob, got {other:?}"),
        }
    }
}
