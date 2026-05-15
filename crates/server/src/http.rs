//! Axum app: peer protocol + local API + embedded UI.
//!
//! Routes:
//! - `/n3ur0n/v0/health`             public liveness probe (id + version)
//! - `/n3ur0n/v0/messages`           signed POST endpoint, dispatched via Node
//! - `/api/v0/health`                local health
//! - `/api/v0/whoami`                returns local instance_id
//! - `/api/v0/peers`                 returns local peer directory
//! - `/api/v0/peers/refresh`         signed describe_self + upsert
//! - `/api/v0/peers/discover`        cascade depth-1
//! - `/api/v0/chat`                  proxies a signed invoke to a chosen peer
//! - `/api/v0/invoke`                generic signed invoke
//! - `/api/v0/conversations*`        conversation CRUD + dispatch (cookie scoped)
//! - `/ui` and `/ui/*`               static HTML chat UI embedded via rust-embed

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Json, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use n3ur0n_core::SignedMessage;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_node::client as peer_client;
use n3ur0n_node::conversation;
use n3ur0n_node::planner::DispatchEvent;
use n3ur0n_node::runtime::NodeRuntime;
use n3ur0n_node::{Node, NodeError, handle_request};
use n3ur0n_storage::conversations as conv_repo;
use n3ur0n_storage::peers as peers_repo;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const META_LIMIT: usize = 16 * 1024;
const INVOKE_LIMIT: usize = 1024 * 1024;
const LOCAL_API_LIMIT: usize = 256 * 1024;
const CLIENT_ID_COOKIE: &str = "n3ur0n_client_id";
const CLIENT_ID_MAX_AGE: u64 = 31_536_000; // 1 year

#[derive(Clone)]
struct AppState {
    node: Node,
    runtime: Option<Arc<NodeRuntime>>,
}

#[derive(Clone, Debug)]
struct ClientId(String);

#[derive(RustEmbed)]
#[folder = "ui/"]
struct UiAssets;

/// Build the HTTP router. `runtime` is `None` when the node has no planner
/// configured — `/api/v0/conversations/:id/messages` returns 503 in that case.
pub fn app(node: Node, runtime: Option<Arc<NodeRuntime>>) -> Router {
    app_with_settings(node, runtime, None)
}

/// Variant of [`app`] that also mounts the settings sub-router under
/// `/api/v0`. The CLI / desktop call this directly so settings routes
/// pass through the same auth middleware stack.
pub fn app_with_settings(
    node: Node,
    runtime: Option<Arc<NodeRuntime>>,
    config_dir: Option<std::path::PathBuf>,
) -> Router {
    use crate::auth::{require_authed, AuthState};
    use crate::require_perm;

    let auth_state = AuthState {
        db: node.db().clone(),
        bypass: crate::auth::read_bypass_env(),
    };
    let state = AppState { node: node.clone(), runtime };

    // Public sub-router: unauthenticated endpoints (health checks, locale
    // catalog, login + bootstrap + logout + auth/me). Everything else
    // requires a session cookie via the `require_authed` layer below.
    let public_api = Router::new()
        .route("/health", get(health))
        .route("/locales", get(api_locales))
        .with_state(state.clone());

    // Routes any logged-in user can hit. Permission-specific guards
    // (caps:write, backends:write, etc.) are layered inside the
    // settings/auth sub-routers.
    let authed_api = Router::new()
        .route("/whoami", get(whoami))
        .route("/caps", get(api_caps).route_layer(require_perm!(crate::auth::perm::CAPS_READ)))
        .route("/peers", get(api_peers).route_layer(require_perm!(crate::auth::perm::PEERS_READ)))
        .route(
            "/peers/refresh",
            post(api_peers_refresh).route_layer(require_perm!(crate::auth::perm::PEERS_WRITE)),
        )
        .route(
            "/peers/discover",
            post(api_peers_discover).route_layer(require_perm!(crate::auth::perm::PEERS_WRITE)),
        )
        .route(
            "/chat",
            post(api_chat)
                .layer(DefaultBodyLimit::max(LOCAL_API_LIMIT))
                .route_layer(require_perm!(crate::auth::perm::CHAT_USE)),
        )
        .route(
            "/invoke",
            post(api_invoke)
                .layer(DefaultBodyLimit::max(LOCAL_API_LIMIT))
                .route_layer(require_perm!(crate::auth::perm::INVOKE_USE)),
        )
        .route("/conversations", get(conv_list).post(conv_create))
        .route(
            "/conversations/:id",
            get(conv_get).patch(conv_patch).delete(conv_delete),
        )
        .route(
            "/conversations/:id/messages",
            post(conv_messages).layer(DefaultBodyLimit::max(LOCAL_API_LIMIT)),
        )
        .route(
            "/conversations/:id/messages/stream",
            post(conv_messages_stream).layer(DefaultBodyLimit::max(LOCAL_API_LIMIT)),
        )
        .route_layer(middleware::from_fn(require_authed))
        .with_state(state.clone());

    let settings_routes = match config_dir {
        Some(dir) => crate::settings::router(dir, node),
        None => Router::new(),
    };

    let api_v0 = Router::new()
        .merge(public_api)
        .merge(authed_api)
        .merge(crate::auth::router(auth_state.clone()))
        .merge(settings_routes)
        .layer(DefaultBodyLimit::max(META_LIMIT))
        .layer(middleware::from_fn(client_id_middleware))
        .layer(middleware::from_fn_with_state(
            auth_state,
            crate::auth::session_middleware,
        ));

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

pub async fn serve(
    addr: SocketAddr,
    node: Node,
    runtime: Option<Arc<NodeRuntime>>,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app(node, runtime)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// client_id middleware (cookie-based, no auth)
// ---------------------------------------------------------------------------

async fn client_id_middleware(mut req: Request, next: Next) -> Response {
    let existing = req
        .headers()
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(parse_client_cookie);

    let (cid, is_new) = match existing {
        Some(v) => (v, false),
        None => (format!("cli_{}", Uuid::new_v4().simple()), true),
    };

    req.extensions_mut().insert(ClientId(cid.clone()));

    let mut resp = next.run(req).await;

    if is_new {
        let cookie = format!(
            "{name}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax",
            name = CLIENT_ID_COOKIE,
            value = cid,
            age = CLIENT_ID_MAX_AGE
        );
        if let Ok(hv) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, hv);
        }
    }

    resp
}

fn parse_client_cookie(raw: &str) -> Option<String> {
    for part in raw.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix(&format!("{CLIENT_ID_COOKIE}=")) {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn extract_client_id(req: &Request) -> Option<&str> {
    req.extensions().get::<ClientId>().map(|c| c.0.as_str())
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

/// List the locale catalogs embedded under `ui/locales/*.json`. Each entry
/// carries the `_meta` block (code, name, native_name) and the relative URL
/// to fetch the full catalog. The frontend uses this to populate the
/// language picker; dropping a new JSON file under `crates/server/ui/locales/`
/// at build time is enough to surface a new language — no server changes.
async fn api_locales() -> impl IntoResponse {
    let mut entries: Vec<Value> = Vec::new();
    for path in UiAssets::iter() {
        let p = path.as_ref();
        if !p.starts_with("locales/") || !p.ends_with(".json") {
            continue;
        }
        let Some(file) = UiAssets::get(p) else { continue };
        let parsed: serde_json::Value = match serde_json::from_slice(&file.data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let meta = parsed.get("_meta").cloned().unwrap_or(json!({}));
        let code = meta
            .get("code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                p.trim_start_matches("locales/")
                    .trim_end_matches(".json")
                    .to_string()
            });
        entries.push(json!({
            "code": code,
            "name": meta.get("name").cloned().unwrap_or(Value::Null),
            "native_name": meta.get("native_name").cloned().unwrap_or(Value::Null),
            "url": format!("/ui/{}", p),
        }));
    }
    entries.sort_by(|a, b| {
        a.get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("code").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Json(json!({
        "available": entries,
        "default": "en",
    }))
    .into_response()
}

/// Local capability registry as JSON. Each entry mirrors `CapabilityDecl`
/// (the wire form returned by `describe_self`) plus a `has_binding` flag
/// so the UI can distinguish manifest-mode caps from legacy compile-time
/// caps.
async fn api_caps(State(state): State<AppState>) -> impl IntoResponse {
    let decls = state.node.registry().all();
    let body: Vec<Value> = decls
        .into_iter()
        .map(|d| {
            let binding = state.node.registry().binding_for(&d.name);
            let has_binding = binding.is_some();
            let binding_type = binding.as_ref().map(|b| b.kind());
            let mut v = serde_json::to_value(&d).unwrap_or(Value::Null);
            if let Value::Object(m) = &mut v {
                m.insert("has_binding".into(), Value::Bool(has_binding));
                if let Some(bt) = binding_type {
                    m.insert("binding_type".into(), Value::String(bt.into()));
                }
            }
            v
        })
        .collect();
    Json(json!({
        "self": state.node.instance_id().as_str(),
        "protocol_version": n3ur0n_core::protocol::PROTOCOL_VERSION,
        "caps": body,
    }))
    .into_response()
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
// Local API: peers
// ---------------------------------------------------------------------------

async fn api_peers(State(state): State<AppState>) -> impl IntoResponse {
    match peers_repo::list(state.node.db(), 200) {
        Ok(rows) => {
            let body: Vec<Value> = rows
                .into_iter()
                .map(|p| {
                    let caps: Vec<Value> = p
                        .describe_self_cached
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .and_then(|d| {
                            d.get("capabilities")
                                .and_then(|c| c.as_array())
                                .cloned()
                        })
                        .unwrap_or_default();
                    let summarised: Vec<Value> = caps
                        .into_iter()
                        .map(|c| {
                            json!({
                                "name": c.get("name").cloned().unwrap_or(Value::Null),
                                "description": c.get("description").cloned().unwrap_or(Value::Null),
                                "schema_in": c.get("schema_in").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect();
                    json!({
                        "instance_id": p.id,
                        "endpoint": p.endpoint,
                        "alias": p.alias,
                        "capabilities": summarised,
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
    peer_endpoint: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    messages: Option<Vec<ChatMessage>>,
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
        state.node.config().endpoint.as_deref(),
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
        state.node.config().endpoint.as_deref(),
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
// Local API: conversations
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateConversationRequest {
    #[serde(default)]
    title: Option<String>,
}

async fn conv_create(
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id"),
    };
    let (_, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, LOCAL_API_LIMIT).await {
        Ok(b) => b,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let payload: CreateConversationRequest = if bytes.is_empty() {
        CreateConversationRequest { title: None }
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, &e.to_string()),
        }
    };

    let title = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    match conversation::create(state.node.db(), &cid, title) {
        Ok(state_obj) => Json(json!({
            "id": state_obj.id,
            "client_id": state_obj.client_id,
            "title": state_obj.title,
            "created_at": state_obj.created_at,
            "updated_at": state_obj.updated_at,
        }))
        .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn conv_list(
    State(state): State<AppState>,
    req: Request,
) -> Response {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id"),
    };
    match conv_repo::list_for_client(state.node.db(), &cid, 200) {
        Ok(rows) => {
            let body: Vec<Value> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "title": r.title,
                        "created_at": r.created_at,
                        "updated_at": r.updated_at,
                    })
                })
                .collect();
            Json(json!({"conversations": body})).into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn conv_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id"),
    };
    match conversation::load(state.node.db(), &id, &cid) {
        Ok(s) => Json(json!({
            "id": s.id,
            "title": s.title,
            "created_at": s.created_at,
            "updated_at": s.updated_at,
            "turns": s.ui_turns(),
        }))
        .into_response(),
        Err(e) => map_conv_load_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct PatchConversationRequest {
    title: Option<String>,
}

async fn conv_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id"),
    };
    let (_, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, LOCAL_API_LIMIT).await {
        Ok(b) => b,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let payload: PatchConversationRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };

    // Ownership check via load()
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    match conversation::load(state.node.db(), &id, &cid) {
        Ok(_) => {
            if let Err(e) = conv_repo::update_meta(
                state.node.db(),
                &id,
                payload.title.as_deref(),
                now,
            ) {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            // Best-effort cache invalidation if runtime is configured.
            if let Some(rt) = &state.runtime {
                let rt = rt.clone();
                let id_clone = id.clone();
                tokio::spawn(async move { rt.evict(&id_clone).await });
            }
            Json(json!({"id": id, "title": payload.title, "updated_at": now})).into_response()
        }
        Err(e) => map_conv_load_error(e),
    }
}

async fn conv_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id"),
    };
    match conversation::load(state.node.db(), &id, &cid) {
        Ok(_) => {
            if let Err(e) = conv_repo::delete(state.node.db(), &id) {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            if let Some(rt) = &state.runtime {
                let rt = rt.clone();
                let id_clone = id.clone();
                tokio::spawn(async move { rt.evict(&id_clone).await });
            }
            (StatusCode::NO_CONTENT, "").into_response()
        }
        Err(e) => map_conv_load_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct ConversationMessageRequest {
    message: String,
}

async fn conv_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let prep = match prepare_dispatch(&state, &id, req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let DispatchPrep { cid, runtime, message } = prep;

    match runtime.handle_user_message(&cid, &id, message).await {
        Ok(outcome) => {
            let trace: Vec<Value> = outcome
                .trace
                .into_iter()
                .map(|t| {
                    json!({
                        "peer_id": t.peer_id,
                        "capability": t.capability,
                        "args": t.args,
                        "result": t.result,
                        "error": t.error,
                    })
                })
                .collect();
            Json(json!({
                "conversation_id": id,
                "reply": outcome.reply,
                "model": outcome.model,
                "trace": trace,
            }))
            .into_response()
        }
        Err(e) => http_error(&e),
    }
}

struct DispatchPrep {
    cid: String,
    runtime: Arc<NodeRuntime>,
    message: String,
}

/// Common pre-flight for both `conv_messages` and `conv_messages_stream`:
/// extract cookie, ensure planner runtime, validate ownership, parse body,
/// auto-title on first user message.
async fn prepare_dispatch(
    state: &AppState,
    id: &str,
    req: Request,
) -> Result<DispatchPrep, Response> {
    let cid = match extract_client_id(&req) {
        Some(v) => v.to_string(),
        None => return Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, "missing client_id")),
    };
    let runtime = match &state.runtime {
        Some(rt) => rt.clone(),
        None => {
            return Err(api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_planner: this node has no planner configured. Use /api/v0/chat for manual mode.",
            ));
        }
    };

    let (_, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, LOCAL_API_LIMIT).await {
        Ok(b) => b,
        Err(e) => return Err(api_error(StatusCode::BAD_REQUEST, &e.to_string())),
    };
    let payload: ConversationMessageRequest = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Err(api_error(StatusCode::BAD_REQUEST, &e.to_string())),
    };
    let message = payload.message.trim().to_string();
    if message.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "message is empty"));
    }

    // Auto-title on first user message if none set.
    match conv_repo::get(state.node.db(), id) {
        Ok(Some(rec)) => {
            if rec.client_id != cid {
                return Err(api_error(StatusCode::NOT_FOUND, "conversation not found"));
            }
            if rec.title.is_none() {
                let title = auto_title(&message, 8);
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let _ = conv_repo::update_meta(state.node.db(), id, Some(&title), now);
            }
        }
        Ok(None) => return Err(api_error(StatusCode::NOT_FOUND, "conversation not found")),
        Err(_) => return Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, "db read failed")),
    }

    Ok(DispatchPrep { cid, runtime, message })
}

async fn conv_messages_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    let prep = match prepare_dispatch(&state, &id, req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let DispatchPrep { cid, runtime, message } = prep;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DispatchEvent>();
    let tx_err = tx.clone();

    let cid_owned = cid.clone();
    let id_owned = id.clone();
    tokio::spawn(async move {
        if let Err(e) = runtime
            .handle_user_message_streaming(&cid_owned, &id_owned, message, tx)
            .await
        {
            let _ = tx_err.send(DispatchEvent::Error { message: e.to_string() });
        }
        // Drop tx_err to close the channel.
        drop(tx_err);
    });

    let stream = UnboundedReceiverStream::new(rx).map(|ev| {
        let event_name = match &ev {
            DispatchEvent::PlanReady { .. } => "plan_ready",
            DispatchEvent::LowConfidence { .. } => "low_confidence",
            DispatchEvent::StepStart { .. } => "step_start",
            DispatchEvent::StepDone { .. } => "step_done",
            DispatchEvent::Reflecting => "reflecting",
            DispatchEvent::Final { .. } => "final",
            DispatchEvent::Error { .. } => "error",
        };
        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
        Ok::<_, std::convert::Infallible>(Event::default().event(event_name).data(data))
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn map_conv_load_error(e: n3ur0n_node::conversation::ConversationError) -> Response {
    use n3ur0n_node::conversation::ConversationError;
    match e {
        ConversationError::NotFound(_) | ConversationError::OwnershipMismatch => {
            api_error(StatusCode::NOT_FOUND, "conversation not found")
        }
        other => api_error(StatusCode::INTERNAL_SERVER_ERROR, &other.to_string()),
    }
}

fn auto_title(message: &str, max_words: usize) -> String {
    message
        .split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
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
