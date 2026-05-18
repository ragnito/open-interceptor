//! Axum HTTP server that fronts the upstream providers.
//!
//! Receives requests on `127.0.0.1:<port>` exactly as Claude Code (or any
//! Anthropic-API client) would send them, peeks at the body to extract the
//! `model` field, hands off to the `Router` to pick a provider, and then
//! dispatches.
//!
//! T1.5 / T1.6 stand up the server skeleton and the two POST handlers
//! (`/v1/messages` and `/v1/messages/count_tokens`). The actual dispatch
//! to upstream lives in `crate::providers` and is wired in T1.7+.

use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use tokio::net::TcpListener;

use crate::domain::config::ProviderType;
use crate::services::{health, models};
use crate::providers;
use crate::router::Router;

/// Shared application state, passed to every handler via Axum's `State`.
#[derive(Clone)]
struct AppState {
    router: Arc<Router>,
    models: Arc<models::ModelsState>,
    health: Arc<health::HealthState>,
}

/// Bind the listener and run the HTTP server until cancelled.
pub async fn serve(router: Arc<Router>) -> anyhow::Result<()> {
    let port = router.port();
    let addr = format!("127.0.0.1:{port}");

    let state = AppState {
        health: Arc::new(health::HealthState::new(router.clone())),
        models: Arc::new(models::ModelsState::new(router.clone())),
        router: router.clone(),
    };

    let app = axum::Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/v1/messages/count_tokens", post(handle_count_tokens))
        .route("/v1/models", get(handle_models))
        .route("/healthz", get(handle_healthz))
        .with_state(state);

    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind {addr}: {e}. Is the port already in use?"))?;

    tracing::info!(addr = %addr, "open-interceptor listening");
    for line in router.route_summaries() {
        tracing::info!("  route: {line}");
    }

    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow::anyhow!("axum::serve failed: {e}"))?;
    Ok(())
}

/// Handler for the main messages endpoint. Extracts the model, picks a
/// provider, and dispatches.
async fn handle_messages(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(state.router, method, uri, headers, body).await
}

async fn handle_count_tokens(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(state.router, method, uri, headers, body).await
}

async fn handle_models(State(state): State<AppState>) -> Response {
    models::handle_models(axum::extract::State(state.models))
        .await
        .into_response()
}

async fn handle_healthz(State(state): State<AppState>) -> Response {
    health::handle_healthz(axum::extract::State(state.health)).await
}

/// Shared dispatch logic between `/v1/messages` and `/v1/messages/count_tokens`.
async fn dispatch(
    router: Arc<Router>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start = std::time::Instant::now();
    let endpoint = uri.path().to_string();

    // Parse just enough of the body to get the model id. The full body
    // continues opaque so we can forward it byte-for-byte upstream.
    let model = match extract_model(&body) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, endpoint = %endpoint, "rejecting request with unparseable body");
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("could not extract `model` from request body: {e}"),
            );
        }
    };

    let resolution = match router.resolve(&model) {
        Some(r) => r,
        None => {
            tracing::warn!(model = %model, endpoint = %endpoint, "no route matches");
            return anthropic_error(
                StatusCode::NOT_FOUND,
                "not_found_error",
                &format!("no route matches model `{model}`"),
            );
        }
    };

    let provider_name = resolution.provider_name.to_string();
    let provider_type = resolution.provider.provider_type;
    let passthrough_auth = resolution.provider.passthrough_auth;
    let effective_model = resolution.effective_model.clone();
    let body_bytes = body.len();

    // Dispatch to the right provider. Each provider has its own error
    // type, so we normalize to a uniform Response here.
    let response = match provider_type {
        ProviderType::AnthropicCompatible => {
            match providers::anthropic::forward(
                resolution.provider,
                &model,
                &effective_model,
                method.clone(),
                uri.clone(),
                headers,
                body,
            )
            .await
            {
                Ok(r) => Ok(r),
                Err(e) => Err(map_anthropic_error(&provider_name, e)),
            }
        }
        ProviderType::OpenaiCompatible => {
            match providers::openai::forward(resolution.provider, &model, &effective_model, body)
                .await
            {
                Ok(r) => Ok(r),
                Err(e) => Err(map_openai_error(&provider_name, e)),
            }
        }
        ProviderType::Passthrough => {
            return anthropic_error(
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
                "passthrough provider type not implemented yet",
            );
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;

    match response {
        Ok(response) => {
            tracing::info!(
                method = %method,
                path = %endpoint,
                model = %model,
                effective_model = %effective_model,
                provider = %provider_name,
                provider_type = ?provider_type,
                passthrough_auth,
                body_bytes,
                upstream_status = response.status().as_u16(),
                elapsed_ms,
                "dispatch ok",
            );
            response
        }
        Err(error_response) => {
            tracing::error!(
                method = %method,
                path = %endpoint,
                model = %model,
                provider = %provider_name,
                upstream_status = error_response.status().as_u16(),
                elapsed_ms,
                "dispatch failed",
            );
            error_response
        }
    }
}

/// Pull only the `model` field from a JSON body. Tolerant of extra fields
/// — we only care about that one string here, the full body stays opaque.
fn extract_model(body: &Bytes) -> Result<String, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct ModelOnly {
        model: String,
    }
    let parsed: ModelOnly = serde_json::from_slice(body)?;
    Ok(parsed.model)
}

/// Build an Anthropic-shaped error response. Same shape Anthropic itself
/// returns, so Claude Code's error handling treats these like any other
/// upstream failure rather than choking on a foreign format.
fn anthropic_error(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "type": "error",
            "error": {
                "type": kind,
                "message": message,
            }
        })),
    )
        .into_response()
}

/// Translate a `providers::anthropic::ForwardError` into the Anthropic-
/// shaped error response the client receives.
fn map_anthropic_error(provider_name: &str, err: providers::anthropic::ForwardError) -> Response {
    use providers::anthropic::ForwardError as E;
    match err {
        E::Upstream(e) => {
            // Distinguish transport failures: timeout → 504, everything else → 502.
            let (status, kind) = if e.is_timeout() {
                (StatusCode::GATEWAY_TIMEOUT, "timeout_error")
            } else {
                (StatusCode::BAD_GATEWAY, "api_error")
            };
            anthropic_error(status, kind, &format!("upstream `{provider_name}` failed: {e}"))
        }
        E::MissingApiKey => anthropic_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "config_error",
            &format!("`{provider_name}` has neither passthrough_auth nor api_key"),
        ),
        other => anthropic_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("forwarding to `{provider_name}` failed: {other}"),
        ),
    }
}

/// Translate a `providers::openai::ForwardError` similarly.
fn map_openai_error(
    provider_name: &str,
    err: providers::openai::ForwardError,
) -> Response {
    use providers::openai::ForwardError as E;
    match err {
        E::MissingApiKey => anthropic_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "config_error",
            &format!("`{provider_name}` is openai_compatible and needs an api_key"),
        ),
        E::UpstreamError { status, body } => {
            tracing::warn!(
                provider = %provider_name,
                upstream_status = status,
                body = %body.chars().take(500).collect::<String>(),
                "upstream error response"
            );
            anthropic_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
                "api_error",
                &format!("upstream `{provider_name}` returned {status}: {}", &body.chars().take(300).collect::<String>()),
            )
        }
        E::Upstream(e) => {
            let (status, kind) = if e.is_timeout() {
                (StatusCode::GATEWAY_TIMEOUT, "timeout_error")
            } else {
                (StatusCode::BAD_GATEWAY, "api_error")
            };
            anthropic_error(status, kind, &format!("upstream `{provider_name}` transport error: {e}"))
        }
        E::RequestParse(e) => anthropic_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            &format!("could not parse request body as Anthropic Messages: {e}"),
        ),
        other => anthropic_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &format!("translating to `{provider_name}` failed: {other}"),
        ),
    }
}
