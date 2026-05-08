//! Axum app: peer protocol + local API.
//!
//! Layout:
//! - `/n3ur0n/v0/messages` — single POST endpoint accepting a `SignedMessage`,
//!   dispatched through [`n3ur0n_node::handle_request`]. Replies are signed
//!   envelopes addressed back to the caller.
//! - `/n3ur0n/v0/health` — unauthenticated liveness probe.
//! - `/api/v0/health` — local health for UI clients.
//! - `/api/v0/whoami` — local convenience returning the canonical id.
//!
//! Strict body-size limits are applied per architecture spec §13.2: 16 KiB on
//! meta routes, 1 MiB on the messages endpoint.

use std::net::SocketAddr;

use anyhow::Result;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use n3ur0n_core::SignedMessage;
use n3ur0n_node::{Node, NodeError, handle_request};
use serde_json::json;
use tower_http::trace::TraceLayer;

const META_LIMIT: usize = 16 * 1024;
const INVOKE_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
struct AppState {
    node: Node,
}

/// Build the HTTP router for a given [`Node`]. Exposed so integration tests
/// can mount it without binding a TCP listener.
pub fn app(node: Node) -> Router {
    let state = AppState { node };

    let api_v0 = Router::new()
        .route("/health", get(health))
        .route("/whoami", get(whoami))
        .layer(DefaultBodyLimit::max(META_LIMIT))
        .with_state(state.clone());

    let proto_v0 = Router::new()
        .route("/health", get(health))
        .route(
            "/messages",
            post(post_message).layer(DefaultBodyLimit::max(INVOKE_LIMIT)),
        )
        .with_state(state);

    Router::new()
        .nest("/api/v0", api_v0)
        .nest("/n3ur0n/v0", proto_v0)
        .layer(TraceLayer::new_for_http())
}

pub async fn serve(addr: SocketAddr, node: Node) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app(node)).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn whoami(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({"instance_id": state.node.instance_id().as_str()}))
}

async fn post_message(
    State(state): State<AppState>,
    Json(msg): Json<SignedMessage>,
) -> impl IntoResponse {
    match handle_request(&state.node, msg).await {
        Ok(reply) => Json(reply).into_response(),
        Err(e) => http_error(&e),
    }
}

fn http_error(err: &NodeError) -> axum::response::Response {
    let (status, kind): (StatusCode, &str) = match err {
        NodeError::Replay => (StatusCode::CONFLICT, "replay"),
        NodeError::UnknownCapability(_) => (StatusCode::NOT_FOUND, "unknown_capability"),
        NodeError::InvalidPayload(_) => (StatusCode::BAD_REQUEST, "invalid_payload"),
        NodeError::Core(c) => match c {
            n3ur0n_core::CoreError::SignatureInvalid => (StatusCode::UNAUTHORIZED, "signature"),
            n3ur0n_core::CoreError::RecipientMismatch { .. } => {
                (StatusCode::BAD_REQUEST, "recipient_mismatch")
            }
            n3ur0n_core::CoreError::TimestampOutOfWindow => {
                (StatusCode::BAD_REQUEST, "timestamp_out_of_window")
            }
            _ => (StatusCode::BAD_REQUEST, "bad_request"),
        },
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    };
    let body = json!({
        "error": kind,
        "message": err.to_string(),
    });
    (status, Json(body)).into_response()
}
