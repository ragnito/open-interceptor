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
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use serde_json::json;
use tokio::net::TcpListener;

use crate::router::Router;

/// Bind the listener and run the HTTP server until cancelled.
pub async fn serve(router: Arc<Router>) -> anyhow::Result<()> {
    let port = router.port();
    let addr = format!("127.0.0.1:{port}");

    let app = axum::Router::new()
        .route("/v1/messages", post(handle_messages))
        .route("/v1/messages/count_tokens", post(handle_count_tokens))
        // /v1/models is part of Phase 2 — stub now so Claude Code's
        // model-discovery probe doesn't blow up.
        .route("/v1/models", get(handle_models_stub))
        .with_state(router.clone());

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
/// provider, and (for now) returns 501 Not Implemented with an Anthropic-
/// shaped error body. T1.7 swaps the stub for real dispatch.
async fn handle_messages(
    State(router): State<Arc<Router>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(router, method, uri, headers, body, "/v1/messages").await
}

async fn handle_count_tokens(
    State(router): State<Arc<Router>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    dispatch(router, method, uri, headers, body, "/v1/messages/count_tokens").await
}

/// Stub for the /v1/models endpoint. Returns the union of every provider's
/// declared `models` list, Anthropic-shape. The dynamic-fetch fallback for
/// providers without a `models:` field is Phase 2 (T2.3).
async fn handle_models_stub(State(router): State<Arc<Router>>) -> Response {
    let data: Vec<_> = router
        .providers()
        .flat_map(|(_, p)| p.models.iter().cloned())
        .map(|id| {
            json!({
                "type": "model",
                "id": id,
                "display_name": id,
            })
        })
        .collect();
    Json(json!({ "data": data, "first_id": null, "last_id": null, "has_more": false })).into_response()
}

/// Shared dispatch logic between `/v1/messages` and `/v1/messages/count_tokens`.
async fn dispatch(
    router: Arc<Router>,
    method: Method,
    uri: Uri,
    _headers: HeaderMap,
    body: Bytes,
    endpoint: &'static str,
) -> Response {
    let start = std::time::Instant::now();

    // Parse just enough of the body to get the model id. The full body
    // continues opaque so we can forward it byte-for-byte upstream.
    let model = match extract_model(&body) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, endpoint, "rejecting request with unparseable body");
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
            tracing::warn!(model = %model, endpoint, "no route matches");
            return anthropic_error(
                StatusCode::NOT_FOUND,
                "not_found_error",
                &format!("no route matches model `{model}`"),
            );
        }
    };

    tracing::info!(
        method = %method,
        path = uri.path(),
        endpoint,
        model = %model,
        effective_model = %resolution.effective_model,
        provider = %resolution.provider_name,
        provider_type = ?resolution.provider.provider_type,
        passthrough_auth = resolution.provider.passthrough_auth,
        body_bytes = body.len(),
        elapsed_us = start.elapsed().as_micros() as u64,
        "dispatch",
    );

    // T1.7 will replace this with:
    //   crate::providers::anthropic::forward(...).await
    // depending on resolution.provider.provider_type.
    anthropic_error(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        &format!(
            "open-interceptor: provider `{}` dispatch not yet implemented (T1.7)",
            resolution.provider_name
        ),
    )
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
