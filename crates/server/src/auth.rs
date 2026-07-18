//! RBAC: session cookie auth + role/permission policy + auth routes.
//!
//! Layering:
//!   - storage::auth handles credential + session persistence (typeless).
//!   - this module owns the *policy* (role → permission set) + the HTTP
//!     glue (cookies, middleware, routes).
//!
//! Three roles, fixed permission table. Adding a new role = edit this file.
//!
//! Permission keys are dot-namespaced (`caps:write`, `backends:read`, …).
//! Each guarded route declares the single key it requires; the middleware
//! resolves the caller's `Role` against the table.

use axum::extract::{Path as AxumPath, Request, State as AxumState};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use n3ur0n_storage::Db;
use n3ur0n_storage::auth::{self as auth_store, Role, UserRecord};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

const SESSION_COOKIE: &str = "n3ur0n_session";
const SESSION_TTL_SECS: i64 = 7 * 24 * 60 * 60; // 7 days sliding
/// Refresh the cookie expiry only when more than this many seconds passed
/// since the last refresh — avoids hammering the DB on every request.
const SESSION_REFRESH_INTERVAL: i64 = 60 * 60;

/// Loaded once at router construction. Carries the DB handle the auth
/// routes + middleware need; everything else stays stateless.
#[derive(Clone)]
pub struct AuthState {
    pub db: Db,
    /// When true (env `N3UR0N_AUTH_DISABLE=1`), the middleware injects a
    /// synthetic Admin user. Loopback-dev convenience only.
    pub bypass: bool,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("bypass", &self.bypass)
            .finish()
    }
}

/// Request extension marker for the authenticated user. Routes that need
/// to identify the caller pull this out via `Extension<AuthedUser>`.
#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub id: i64,
    pub username: String,
    pub role: Role,
    /// `None` when the bypass flag synthesised this user (no real session
    /// to refresh / revoke).
    pub session_token_hash: Option<String>,
}

/// Permission keys the runtime understands. Kept in one place so the UI
/// can render a static list if it ever wants to.
pub mod perm {
    pub const CHAT_USE: &str = "chat:use";
    pub const INVOKE_USE: &str = "invoke:use";
    pub const CAPS_READ: &str = "caps:read";
    pub const CAPS_WRITE: &str = "caps:write";
    pub const PEERS_READ: &str = "peers:read";
    pub const PEERS_WRITE: &str = "peers:write";
    pub const BACKENDS_READ: &str = "backends:read";
    pub const BACKENDS_WRITE: &str = "backends:write";
    pub const USERS_READ: &str = "users:read";
    pub const USERS_WRITE: &str = "users:write";
    pub const IDENTITY_READ: &str = "identity:read";
    pub const IDENTITY_ROTATE: &str = "identity:rotate";
    pub const FILES_READ: &str = "files:read";
    pub const FILES_DELETE: &str = "files:delete";
    pub const CAPS_BLOBS_READ: &str = "caps:blobs:read";
}

/// Permissions granted to each role. The lowest-tier User can chat +
/// invoke + browse; Operator adds cap CRUD; Admin adds everything else.
pub fn permissions_for(role: Role) -> &'static [&'static str] {
    use perm::*;
    match role {
        Role::User => &[
            CHAT_USE,
            INVOKE_USE,
            CAPS_READ,
            PEERS_READ,
            BACKENDS_READ,
            IDENTITY_READ,
            FILES_READ,
            FILES_DELETE,
        ],
        Role::Operator => &[
            CHAT_USE,
            INVOKE_USE,
            CAPS_READ,
            CAPS_WRITE,
            PEERS_READ,
            BACKENDS_READ,
            IDENTITY_READ,
            FILES_READ,
            FILES_DELETE,
            CAPS_BLOBS_READ,
        ],
        Role::Admin => &[
            CHAT_USE,
            INVOKE_USE,
            CAPS_READ,
            CAPS_WRITE,
            PEERS_READ,
            PEERS_WRITE,
            BACKENDS_READ,
            BACKENDS_WRITE,
            USERS_READ,
            USERS_WRITE,
            IDENTITY_READ,
            IDENTITY_ROTATE,
            FILES_READ,
            FILES_DELETE,
            CAPS_BLOBS_READ,
        ],
    }
}

pub fn has_permission(role: Role, want: &str) -> bool {
    permissions_for(role).contains(&want)
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Resolve the caller's session cookie → `AuthedUser` extension. Missing
/// or invalid cookie leaves the extension absent (anonymous request);
/// guard layers downstream produce 401.
pub async fn session_middleware(
    AxumState(state): AxumState<AuthState>,
    mut req: Request,
    next: Next,
) -> Response {
    if state.bypass {
        req.extensions_mut().insert(AuthedUser {
            id: 0,
            username: "dev-bypass".into(),
            role: Role::Admin,
            session_token_hash: None,
        });
        return next.run(req).await;
    }

    let cookie_token = extract_cookie(req.headers().get(header::COOKIE));
    let mut refreshed: Option<String> = None;
    if let Some(token) = cookie_token.as_deref() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let hash = auth_store::hash_session_token(token);
        if let Ok(Some(user)) = auth_store::lookup_session(&state.db, &hash, now) {
            // Sliding refresh on every authed hit. The refresh-interval
            // constant is intentionally permissive — DB write is cheap
            // and we want consistent expiry semantics for now.
            let exp = now + SESSION_TTL_SECS;
            let _ = auth_store::refresh_session_expiry(&state.db, &hash, exp);
            refreshed = Some(token.to_string());
            req.extensions_mut().insert(AuthedUser {
                id: user.id,
                username: user.username,
                role: user.role,
                session_token_hash: Some(hash),
            });
        }
    }

    let mut resp = next.run(req).await;
    if let Some(token) = refreshed
        && let Ok(hv) = HeaderValue::from_str(&session_cookie_string(&token, SESSION_TTL_SECS))
    {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    let _ = SESSION_REFRESH_INTERVAL;
    resp
}

fn extract_cookie(header: Option<&HeaderValue>) -> Option<String> {
    let raw = header.and_then(|v| v.to_str().ok())?;
    for part in raw.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix(&format!("{SESSION_COOKIE}="))
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

fn session_cookie_string(token: &str, max_age: i64) -> String {
    format!(
        "{name}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax",
        name = SESSION_COOKIE,
        value = token,
        age = max_age
    )
}

fn cleared_session_cookie_string() -> String {
    format!(
        "{name}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax",
        name = SESSION_COOKIE
    )
}

/// Middleware: 401s if no authenticated user. No permission check —
/// pair with this on routes everyone-logged-in can hit (chat, invoke,
/// browse).
pub async fn require_authed(req: Request, next: Next) -> Response {
    if req.extensions().get::<AuthedUser>().is_none() {
        return unauthorised();
    }
    next.run(req).await
}

/// Middleware: 401s unless the caller has the named permission. Use via
/// `.route_layer(axum::middleware::from_fn(|req, next| require(req, next,
/// perm::CAPS_WRITE)))` — but the convenience helper below sugars it.
pub async fn require_permission(req: Request, next: Next, permission: &'static str) -> Response {
    let user = req.extensions().get::<AuthedUser>().cloned();
    match user {
        None => unauthorised(),
        Some(u) if has_permission(u.role, permission) => next.run(req).await,
        Some(_) => forbidden(permission),
    }
}

/// Convenience: build a layer that enforces a single permission.
#[macro_export]
macro_rules! require_perm {
    ($perm:expr) => {
        axum::middleware::from_fn(
            |req: axum::extract::Request, next: axum::middleware::Next| async move {
                $crate::auth::require_permission(req, next, $perm).await
            },
        )
    };
}

fn unauthorised() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthenticated"})),
    )
        .into_response()
}
fn forbidden(perm: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": "forbidden", "required_permission": perm})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub fn router(state: AuthState) -> Router {
    let users_routes = Router::new()
        .route("/users", get(list_users).post(create_user_route))
        .route(
            "/users/{id}",
            axum::routing::patch(update_user_route).delete(delete_user_route),
        )
        .route_layer(require_perm!(perm::USERS_WRITE))
        .with_state(state.clone());
    Router::new()
        .route("/auth/bootstrap", post(bootstrap))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/password", post(change_password))
        .with_state(state)
        .merge(users_routes)
}

#[derive(Debug, Deserialize)]
struct CredsRequest {
    username: String,
    password: String,
}

async fn bootstrap(
    AxumState(state): AxumState<AuthState>,
    Json(req): Json<CredsRequest>,
) -> Response {
    let n = match auth_store::count_users(&state.db) {
        Ok(n) => n,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if n > 0 {
        return error(StatusCode::CONFLICT, "bootstrap already completed");
    }
    if !is_valid_username(&req.username) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid username (3-32 chars, [A-Za-z0-9._-])",
        );
    }
    if req.password.chars().count() < 6 {
        return error(StatusCode::BAD_REQUEST, "password must be at least 6 chars");
    }
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let user =
        match auth_store::create_user(&state.db, &req.username, &req.password, Role::Admin, now) {
            Ok(u) => u,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
    issue_session(&state.db, user, now)
}

async fn login(AxumState(state): AxumState<AuthState>, Json(req): Json<CredsRequest>) -> Response {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let stored = match auth_store::fetch_password_hash(&state.db, &req.username) {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid credentials"),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let (user_id, hash) = stored;
    let ok = auth_store::verify_password(&req.password, &hash).unwrap_or(false);
    if !ok {
        return error(StatusCode::UNAUTHORIZED, "invalid credentials");
    }
    let user = match auth_store::get_user_by_id(&state.db, user_id) {
        Ok(Some(u)) => u,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "user lookup failed"),
    };
    let _ = auth_store::touch_last_login(&state.db, user.id, now);
    issue_session(&state.db, user, now)
}

fn issue_session(db: &Db, user: UserRecord, now: i64) -> Response {
    let (token, hash) = auth_store::mint_session_token();
    let expires = now + SESSION_TTL_SECS;
    if let Err(e) = auth_store::create_session(db, user.id, &hash, now, expires) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let body = json!({
        "id": user.id,
        "username": user.username,
        "role": user.role,
        "permissions": permissions_for(user.role),
    });
    let mut resp = (StatusCode::OK, Json(body)).into_response();
    if let Ok(hv) = HeaderValue::from_str(&session_cookie_string(&token, SESSION_TTL_SECS)) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp
}

async fn logout(
    AxumState(state): AxumState<AuthState>,
    user: Option<Extension<AuthedUser>>,
) -> Response {
    if let Some(Extension(u)) = user
        && let Some(hash) = u.session_token_hash
    {
        let _ = auth_store::delete_session(&state.db, &hash);
    }
    let mut resp = (StatusCode::OK, Json(json!({"ok": true}))).into_response();
    if let Ok(hv) = HeaderValue::from_str(&cleared_session_cookie_string()) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp
}

async fn me(
    AxumState(state): AxumState<AuthState>,
    user: Option<Extension<AuthedUser>>,
) -> Response {
    let body = match user {
        Some(Extension(u)) => json!({
            "authenticated": true,
            "id": u.id,
            "username": u.username,
            "role": u.role,
            "permissions": permissions_for(u.role),
            "bootstrap_required": false,
        }),
        None => {
            let count = auth_store::count_users(&state.db).unwrap_or(0);
            json!({
                "authenticated": false,
                "bootstrap_required": count == 0,
            })
        }
    };
    Json(body).into_response()
}

#[derive(Debug, Deserialize)]
struct PasswordChangeRequest {
    current_password: String,
    new_password: String,
}

async fn change_password(
    AxumState(state): AxumState<AuthState>,
    user: Option<Extension<AuthedUser>>,
    Json(req): Json<PasswordChangeRequest>,
) -> Response {
    let Some(Extension(u)) = user else {
        return unauthorised();
    };
    if req.new_password.chars().count() < 6 {
        return error(
            StatusCode::BAD_REQUEST,
            "new password must be at least 6 chars",
        );
    }
    let (_, hash) = match auth_store::fetch_password_hash(&state.db, &u.username) {
        Ok(Some(v)) => v,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "user not found"),
    };
    if !auth_store::verify_password(&req.current_password, &hash).unwrap_or(false) {
        return error(StatusCode::UNAUTHORIZED, "current password incorrect");
    }
    if let Err(e) = auth_store::set_user_password(&state.db, u.id, &req.new_password) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    // Revoke other sessions; keep the current one alive.
    if let Err(e) = auth_store::delete_sessions_for_user(&state.db, u.id) {
        tracing::warn!(error=%e, "password change: session purge failed");
    }
    Json(json!({"ok": true})).into_response()
}

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    role: String,
}

async fn list_users(AxumState(state): AxumState<AuthState>) -> Response {
    match auth_store::list_users(&state.db) {
        Ok(list) => Json(json!({ "users": list })).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn create_user_route(
    AxumState(state): AxumState<AuthState>,
    Json(req): Json<CreateUserRequest>,
) -> Response {
    if !is_valid_username(&req.username) {
        return error(StatusCode::BAD_REQUEST, "invalid username");
    }
    if req.password.chars().count() < 6 {
        return error(StatusCode::BAD_REQUEST, "password must be at least 6 chars");
    }
    let Some(role) = Role::parse(&req.role) else {
        return error(StatusCode::BAD_REQUEST, "role must be user|operator|admin");
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match auth_store::create_user(&state.db, &req.username, &req.password, role, now) {
        Ok(u) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(u).unwrap_or(Value::Null)),
        )
            .into_response(),
        Err(e) => error(StatusCode::CONFLICT, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateUserRequest {
    role: Option<String>,
    password: Option<String>,
}

async fn update_user_route(
    AxumState(state): AxumState<AuthState>,
    AxumPath(id): AxumPath<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Response {
    if let Some(r) = req.role.as_deref() {
        let Some(role) = Role::parse(r) else {
            return error(StatusCode::BAD_REQUEST, "role must be user|operator|admin");
        };
        if let Err(e) = auth_store::update_user_role(&state.db, id, role) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }
    if let Some(pw) = req.password.as_deref() {
        if pw.chars().count() < 6 {
            return error(StatusCode::BAD_REQUEST, "password must be at least 6 chars");
        }
        if let Err(e) = auth_store::set_user_password(&state.db, id, pw) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        let _ = auth_store::delete_sessions_for_user(&state.db, id);
    }
    let body = auth_store::get_user_by_id(&state.db, id)
        .ok()
        .flatten()
        .map(|u| serde_json::to_value(u).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    Json(json!({"ok": true, "user": body})).into_response()
}

async fn delete_user_route(
    AxumState(state): AxumState<AuthState>,
    AxumPath(id): AxumPath<i64>,
    user: Option<Extension<AuthedUser>>,
) -> Response {
    // Refuse self-delete to avoid locking the publisher out.
    if let Some(Extension(u)) = user.as_ref()
        && u.id == id
    {
        return error(StatusCode::BAD_REQUEST, "cannot delete yourself");
    }
    // Refuse deleting the last admin.
    if let Ok(list) = auth_store::list_users(&state.db) {
        let admins: Vec<_> = list.iter().filter(|u| u.role == Role::Admin).collect();
        if admins.len() <= 1 && admins.iter().any(|u| u.id == id) {
            return error(StatusCode::BAD_REQUEST, "cannot delete the last admin");
        }
    }
    if let Err(e) = auth_store::delete_user(&state.db, id) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({"ok": true})).into_response()
}

fn error(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"error": msg}))).into_response()
}

fn is_valid_username(s: &str) -> bool {
    let n = s.chars().count();
    if !(3..=32).contains(&n) {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Helper: read the bypass flag from env so the boot code doesn't repeat
/// the literal.
pub fn read_bypass_env() -> bool {
    std::env::var("N3UR0N_AUTH_DISABLE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
