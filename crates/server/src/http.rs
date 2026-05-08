//! Axum app: peer protocol + local API + embedded UI.
//!
//! Routes:
//! - `/n3ur0n/v0/health`             public liveness probe (id + version)
//! - `/n3ur0n/v0/messages`           signed POST endpoint, dispatched via Node
//! - `/api/v0/health`                local health
//! - `/api/v0/whoami`                returns local instance_id
//! - `/api/v0/peers`                 returns local peer directory
//! - `/api/v0/chat`                  proxies a signed invoke to a chosen peer
//! - `/ui` and `/ui/*`               static HTML chat UI embedded via rust-embed

use std::net::SocketAddr;

use anyhow::Result;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Json, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use n3ur0n_core::SignedMessage;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_node::client as peer_client;
use n3ur0n_node::{Node, NodeError, handle_request};
use n3ur0n_storage::peers as peers_repo;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

const META_LIMIT: usize = 16 * 1024;
const INVOKE_LIMIT: usize = 1024 * 1024;
const LOCAL_API_LIMIT: usize = 256 * 1024;

#[derive(Clone)]
struct AppState {
    node: Node,
}

#[derive(RustEmbed)]
#[folder = "ui/"]
struct UiAssets;

/// Build the HTTP router for a given [`Node`]. Exposed so integration tests
/// can mount it without binding a TCP listener.
pub fn app(node: Node) -> Router {
    let state = AppState { node };

    let api_v0 = Router::new()
        .route("/health", get(health))
        .route("/whoami", get(whoami))
        .route("/peers", get(api_peers))
        .route("/peers/refresh", post(api_peers_refresh))
        .route("/peers/discover", post(api_peers_discover))
        .route(
            "/chat",
            post(api_chat).layer(DefaultBodyLimit::max(LOCAL_API_LIMIT)),
        )
        .route(
            "/invoke",
            post(api_invoke).layer(DefaultBodyLimit::max(LOCAL_API_LIMIT)),
        )
        .layer(DefaultBodyLimit::max(META_LIMIT))
        .with_state(state.clone());

    let proto_v0 = Router::new()
        .route("/health", get(public_health))
        .route(
            "/messages",
            post(post_message).layer(DefaultBodyLimit::max(INVOKE_LIMIT)),
        )
        .with_state(state);

    Router::new()
        .nest("/api/v0", api_v0)
        .nest("/n3ur0n/v0", proto_v0)
        .route("/", get(|| async { Redirect::permanent("/ui/") }))
        .route("/ui", get(ui_index))
        .route("/ui/", get(ui_index))
        .route("/ui/*path", get(ui_static))
        .layer(TraceLayer::new_for_http())
}

pub async fn serve(addr: SocketAddr, node: Node) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app(node)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public protocol routes
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

async fn public_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "instance_id": state.node.instance_id().as_str(),
        "protocol_version": n3ur0n_core::protocol::PROTOCOL_VERSION,
    }))
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

// ---------------------------------------------------------------------------
// Local UI / API routes
// ---------------------------------------------------------------------------

async fn api_peers(State(state): State<AppState>) -> impl IntoResponse {
    match peers_repo::list(state.node.db(), 200) {
        Ok(rows) => {
            let body: Vec<Value> = rows
                .into_iter()
                .map(|p| {
                    let caps: Vec<String> = p
                        .describe_self_cached
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .and_then(|d| {
                            d.get("capabilities")
                                .and_then(|c| c.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|c| {
                                            c.get("name").and_then(|n| n.as_str()).map(String::from)
                                        })
                                        .collect()
                                })
                        })
                        .unwrap_or_default();
                    json!({
                        "instance_id": p.id,
                        "endpoint": p.endpoint,
                        "alias": p.alias,
                        "capabilities": caps,
                    })
                })
                .collect();
            Json(json!({ "self": state.node.instance_id().as_str(), "peers": body })).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    /// Endpoint URL of the peer to query.
    peer_endpoint: String,
    /// Single-shot prompt; mutually exclusive with `messages`.
    #[serde(default)]
    prompt: Option<String>,
    /// Multi-turn conversation history.
    #[serde(default)]
    messages: Option<Vec<ChatMessage>>,
    /// Optional override of the default model on the remote peer.
    #[serde(default)]
    model: Option<String>,
}

async fn api_chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let mut args = match (req.prompt, req.messages) {
        (Some(p), None) => json!({"prompt": p}),
        (None, Some(msgs)) if !msgs.is_empty() => {
            let arr: Vec<Value> = msgs
                .into_iter()
                .map(|m| json!({"role": m.role, "content": m.content}))
                .collect();
            json!({"messages": arr})
        }
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "either `prompt` or non-empty `messages` is required",
            );
        }
    };
    if let Some(model) = req.model {
        args["model"] = Value::String(model);
    }
    let payload = json!({
        "capability": "chat",
        "args": args,
    });

    let client = peer_client::http_client();
    let reply = match peer_client::send_signed(
        &client,
        state.node.keypair(),
        &req.peer_endpoint,
        ProtocolVerb::Invoke,
        payload,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    };

    Json(json!({
        "peer_id": reply.envelope.sender_id.as_str(),
        "reply": reply.envelope.payload,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct PeersDiscoverRequest {
    /// Capability name to cascade-search for.
    capability: String,
}

async fn api_peers_discover(
    State(state): State<AppState>,
    Json(req): Json<PeersDiscoverRequest>,
) -> impl IntoResponse {
    match n3ur0n_node::discovery::discover_capability(&state.node, &req.capability).await {
        Ok(added) => Json(json!({"added": added, "capability": req.capability})).into_response(),
        Err(e) => api_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct PeersRefreshRequest {
    endpoint: String,
}

async fn api_peers_refresh(
    State(state): State<AppState>,
    Json(req): Json<PeersRefreshRequest>,
) -> impl IntoResponse {
    let client = peer_client::http_client();
    match n3ur0n_node::discovery::refresh_peer(&state.node, &client, &req.endpoint).await {
        Ok(desc) => Json(json!({
            "instance_id": desc.instance_id.as_str(),
            "endpoint": desc.endpoint,
            "alias": desc.alias,
            "capabilities": desc.capabilities.iter().map(|c| &c.name).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => api_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    peer_endpoint: String,
    capability: String,
    #[serde(default)]
    args: Value,
}

/// Generic capability invoker — feeds whatever JSON `args` to the named
/// capability on the remote peer. Used by the UI's capability picker for
/// non-chat capabilities.
async fn api_invoke(
    State(state): State<AppState>,
    Json(req): Json<InvokeRequest>,
) -> impl IntoResponse {
    let payload = json!({
        "capability": req.capability,
        "args": req.args,
    });
    let client = peer_client::http_client();
    let reply = match peer_client::send_signed(
        &client,
        state.node.keypair(),
        &req.peer_endpoint,
        ProtocolVerb::Invoke,
        payload,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    Json(json!({
        "peer_id": reply.envelope.sender_id.as_str(),
        "reply": reply.envelope.payload,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Embedded UI
// ---------------------------------------------------------------------------

async fn ui_index() -> impl IntoResponse {
    serve_asset("index.html")
}

async fn ui_static(Path(path): Path<String>) -> impl IntoResponse {
    if path.is_empty() {
        return serve_asset("index.html");
    }
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    match UiAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(file.data))
                .expect("response builder")
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("not found"))
            .expect("response builder"),
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

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

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}
