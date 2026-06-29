//! Model → provider routing.
//!
//! Given the `model` field from an incoming request, the router walks the
//! configured routes top-to-bottom and returns the first one whose glob
//! patterns match. Glob compilation happens once at startup via
//! `globset::GlobSet` — the hot path is a single bitset check per route,
//! same crate used by ripgrep.

use std::collections::HashMap;

use globset::{Glob, GlobSet, GlobSetBuilder};

use super::config::{Config, ContextGuard, Provider};

/// Router owns the parsed `Config` and the pre-compiled glob matchers.
/// Designed to live behind an `Arc` and be shared across request handlers.
#[derive(Debug)]
pub struct Router {
    port: u16,
    routes: Vec<CompiledRoute>,
    providers: HashMap<String, Provider>,
    context_guard: Option<ContextGuard>,
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
            context_guard: config.context_guard,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn context_guard(&self) -> Option<ContextGuard> {
        self.context_guard
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
