use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use reqwest::Client;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::domain::config::{Provider, ProviderType};
use crate::domain::router::Router;

const CACHE_TTL: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealth {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResult {
    pub status: &'static str,
    pub providers: HashMap<String, ProviderHealth>,
}

pub struct HealthState {
    router: Arc<Router>,
    cache: RwLock<Option<(Instant, HealthResult)>>,
}

impl HealthState {
    pub fn new(router: Arc<Router>) -> Self {
        Self {
            router,
            cache: RwLock::new(None),
        }
    }
}

pub async fn handle_healthz(State(state): State<Arc<HealthState>>) -> Response {
    {
        let guard = state.cache.read().await;
        if let Some((ts, ref result)) = *guard
            && ts.elapsed() < CACHE_TTL
        {
            return cached_response(result);
        }
    }

    let client = Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .expect("reqwest client");

    let mut providers = HashMap::new();
    let mut all_ok = true;

    for (name, provider) in state.router.providers() {
        let health = probe_provider(&client, provider).await;
        if health.status != "ok" {
            all_ok = false;
        }
        providers.insert(name.to_string(), health);
    }

    let result = HealthResult {
        status: if all_ok { "ok" } else { "degraded" },
        providers,
    };

    let mut guard = state.cache.write().await;
    *guard = Some((Instant::now(), result.clone()));
    drop(guard);

    cached_response(&result)
}

fn cached_response(result: &HealthResult) -> Response {
    let status = if result.status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(result.clone())).into_response()
}

async fn probe_provider(client: &Client, provider: &Provider) -> ProviderHealth {
    // passthrough_auth without a local key means we don't own credentials —
    // skip network probe, report as operational (it's validated by real traffic).
    if provider.passthrough_auth && provider.api_key.is_none() {
        return ProviderHealth {
            status: "ok",
            latency_ms: None,
            error: None,
            note: Some("passthrough auth — not probed"),
        };
    }

    if provider.provider_type == ProviderType::Passthrough {
        return ProviderHealth {
            status: "ok",
            latency_ms: None,
            error: None,
            note: Some("passthrough type — not probed"),
        };
    }

    let probe_url = format!("{}/v1/models", provider.url.trim_end_matches('/'));
    let mut req = client.get(&probe_url);

    match provider.provider_type {
        ProviderType::AnthropicCompatible => {
            if let Some(key) = provider.all_keys().first() {
                req = req
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01");
            }
        }
        ProviderType::OpenaiCompatible => {
            if let Some(key) = provider.all_keys().first() {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
        }
        ProviderType::Passthrough => unreachable!(),
    }

    let start = Instant::now();
    match req.send().await {
        Ok(resp) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            // 401 = reachable but auth rejected; still counts as "up"
            let ok = resp.status().is_success() || resp.status() == StatusCode::UNAUTHORIZED;
            ProviderHealth {
                status: if ok { "ok" } else { "error" },
                latency_ms: Some(latency_ms),
                error: if ok {
                    None
                } else {
                    Some(format!("HTTP {}", resp.status().as_u16()))
                },
                note: None,
            }
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            ProviderHealth {
                status: "error",
                latency_ms: Some(latency_ms),
                error: Some(e.to_string()),
                note: None,
            }
        }
    }
}
