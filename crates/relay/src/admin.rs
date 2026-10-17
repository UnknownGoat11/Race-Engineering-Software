//! Admin listener (ARCHITECTURE.md §7): a separate HTTP listener (default `127.0.0.1:9090`, never
//! the telemetry port) with bearer-token authentication.
//!
//! * `GET /health`: JSON status (store reachability, uptime, sessions, connections). 200 when
//!   healthy, 503 when the store is unreachable.
//! * `GET /metrics`: Prometheus text exposition ([`crate::metrics::Metrics::render`]).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use tokio::net::TcpListener;

use crate::config::environment_name;
use crate::secrets::ct_eq_secret;
use crate::state::RelayState;
use crate::store::SessionStore as _;

/// Log component of this module.
const COMPONENT: &str = "relay.admin";
/// A store ping slower than this counts as unreachable.
const STORE_PING_TIMEOUT: Duration = Duration::from_secs(2);
/// Prometheus text exposition content type.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// Bearer scheme prefix of the Authorization header.
const BEARER_PREFIX: &str = "Bearer ";

/// `/health` body.
#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    db: &'static str,
    uptime_s: u64,
    env: &'static str,
    version: String,
    active_sessions: u64,
    sessions_in_grace: u64,
    engineers_connected: u64,
    active_connections: u64,
}

/// Serves the admin API until the relay shuts down.
pub(crate) async fn serve(state: Arc<RelayState>, listener: TcpListener) {
    let shutdown = state.shutdown.clone();
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .with_state(state);
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
    {
        tracing::error!(component = COMPONENT, error = %e, "admin listener failed");
    }
}

/// Whether the request carries the admin bearer token. Without a configured token (development
/// only, enforced by `RelayConfig::validate`) every local request is allowed.
fn authorized(state: &RelayState, headers: &HeaderMap) -> bool {
    let Some(expected) = &state.config.admin_token else {
        return true;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix(BEARER_PREFIX))
        .is_some_and(|token| ct_eq_secret(token.trim(), expected.expose()))
}

fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

async fn health(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let db = if state.store.is_memory() {
        "memory"
    } else {
        match tokio::time::timeout(STORE_PING_TIMEOUT, state.store.ping()).await {
            Ok(Ok(())) => "up",
            Ok(Err(e)) => {
                tracing::warn!(component = COMPONENT, error = %e, "store ping failed");
                "down"
            }
            Err(_) => "down",
        }
    };
    let gauges = state.registry.gauges();
    let healthy = db != "down";
    let body = Health {
        status: if healthy { "ok" } else { "degraded" },
        db,
        uptime_s: state.uptime().as_secs(),
        env: environment_name(state.config.env),
        version: state.relay_version.clone(),
        active_sessions: gauges.active_sessions,
        sessions_in_grace: gauges.sessions_in_grace,
        engineers_connected: gauges.engineers_connected,
        active_connections: state.metrics.active_connections.get(),
    };
    let code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, axum::Json(body)).into_response()
}

async fn metrics(State(state): State<Arc<RelayState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return unauthorized();
    }
    let text = state.metrics.render(state.registry.gauges());
    let mut response = (StatusCode::OK, text).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
    );
    response
}
