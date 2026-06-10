//! Settings REST surface: backend + capability manifest CRUD over the
//! local filesystem.
//!
//! Endpoints (all bound to whatever listener the host server mounts):
//!
//!   GET    /api/v0/backends                — list parsed backend manifests
//!   POST   /api/v0/backends                — create/overwrite a backend.toml
//!   DELETE /api/v0/backends/:name          — remove a backend.toml
//!
//!   GET    /api/v0/caps/manifests          — list capability manifests
//!   POST   /api/v0/caps/manifests          — upsert a cap.toml from JSON
//!   GET    /api/v0/caps/manifests/:name    — fetch raw cap.toml
//!   DELETE /api/v0/caps/manifests/:name    — remove a cap.toml
//!
//! CRUD operations write to `<config>/backends/<name>.toml` or
//! `<config>/caps/<name>.toml`. Backend and capability CRUD trigger live
//! reloads on the in-memory registries. When the planner uses a named
//! backend, backend saves also hot-reload the planner runtime.
//!
//! Lifted from the desktop shell so the headless server exposes the
//! same Settings surface to the embedded web UI.

use std::path::PathBuf;

use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use n3ur0n_node::manifest::{load_backend_dir, BackendKind as MfBackendKind};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

#[derive(Clone, Debug)]
pub struct SettingsState {
    pub config_dir: PathBuf,
    pub node: n3ur0n_node::Node,
    /// Present when the node was started with a planner; enables hot-reload
    /// after backend manifest edits that affect `planner.toml`.
    pub planner: Option<crate::planner_config::PlannerRuntimeHandle>,
}

pub fn router(
    config_dir: PathBuf,
    node: n3ur0n_node::Node,
    planner: Option<crate::planner_config::PlannerRuntimeHandle>,
) -> Router {
    use crate::auth::perm;
    use crate::require_perm;
    let state = SettingsState {
        config_dir,
        node,
        planner,
    };

    // Mutating backend routes — admin-only.
    let backends_write = Router::new()
        .route("/backends", axum::routing::post(create_backend))
        .route(
            "/backends/:name",
            axum::routing::delete(delete_backend),
        )
        .route_layer(require_perm!(perm::BACKENDS_WRITE))
        .with_state(state.clone());
    // Read-only backend routes — any authenticated user (BACKENDS_READ).
    let backends_read = Router::new()
        .route("/backends", get(list_backends))
        .route("/backends/:name", get(get_backend))
        .route_layer(require_perm!(perm::BACKENDS_READ))
        .with_state(state.clone());

    // Mutating cap manifest routes — operator or admin (CAPS_WRITE).
    let caps_write = Router::new()
        .route("/caps/manifests", axum::routing::post(upsert_cap_manifest))
        .route(
            "/caps/manifests/:name",
            axum::routing::delete(delete_cap_manifest),
        )
        .route_layer(require_perm!(perm::CAPS_WRITE))
        .with_state(state.clone());
    // Read-only cap manifest routes — any authenticated user.
    let caps_read = Router::new()
        .route("/caps/manifests", get(list_cap_manifests))
        .route("/caps/manifests/:name", get(get_cap_manifest))
        .route_layer(require_perm!(perm::CAPS_READ))
        .with_state(state);

    Router::new()
        .merge(backends_write)
        .merge(backends_read)
        .merge(caps_write)
        .merge(caps_read)
}

fn settings_error(status: StatusCode, message: &str) -> axum::response::Response {
    (status, Json(json!({"error": message}))).into_response()
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// Backend CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateBackendRequest {
    name: String,
    kind: String,
    // openai_compat fields
    base_url: Option<String>,
    default_model: Option<String>,
    api_key: Option<String>,
    /// Edit-mode flag: if true and `api_key` is None, the server preserves
    /// the existing on-disk api_key rather than blanking it. Lets the UI
    /// show a masked field that the user can leave empty to keep.
    #[serde(default)]
    api_key_keep: bool,
    // mcp_server fields
    transport: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    // http_base fields (base_url shared with openai_compat above)
    #[serde(default)]
    headers: std::collections::HashMap<String, String>,
}

async fn list_backends(AxumState(state): AxumState<SettingsState>) -> impl IntoResponse {
    let dir = state.config_dir.join("backends");
    let mut out: Vec<Value> = Vec::new();
    for result in load_backend_dir(&dir) {
        match result {
            Ok(m) => {
                let (kind, details) = match &m.kind {
                    MfBackendKind::OpenAICompat(cfg) => (
                        "openai_compat",
                        json!({
                            "base_url": cfg.base_url,
                            "default_model": cfg.default_model,
                            "has_api_key": !cfg.api_key.is_empty(),
                        }),
                    ),
                    MfBackendKind::McpServer(cfg) => (
                        "mcp_server",
                        json!({
                            "transport": format!("{:?}", cfg.transport).to_lowercase(),
                            "command": cfg.command,
                            "args_count": cfg.args.len(),
                        }),
                    ),
                    MfBackendKind::HttpBase(cfg) => (
                        "http_base",
                        json!({
                            "base_url": cfg.base_url,
                            "header_count": cfg.headers.len(),
                        }),
                    ),
                };
                out.push(json!({
                    "name": m.name,
                    "kind": kind,
                    "details": details,
                }));
            }
            Err(e) => {
                out.push(json!({
                    "name": null,
                    "error": e.to_string(),
                }));
            }
        }
    }
    Json(json!({
        "backends": out,
        "dir": dir.display().to_string(),
    }))
    .into_response()
}

async fn create_backend(
    AxumState(state): AxumState<SettingsState>,
    Json(req): Json<CreateBackendRequest>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "name must be non-empty and match [a-zA-Z0-9_-]",
        );
    }
    let body = match req.kind.as_str() {
        "openai_compat" => {
            let Some(base_url_raw) = req.base_url.as_deref() else {
                return settings_error(StatusCode::BAD_REQUEST, "base_url required for openai_compat");
            };
            let base_url = n3ur0n_adapters::openai::normalize_openai_base_url(base_url_raw);
            let Some(default_model) = req.default_model.as_deref() else {
                return settings_error(StatusCode::BAD_REQUEST, "default_model required for openai_compat");
            };
            // Edit-mode preservation: when `api_key_keep` is set + no
            // explicit key supplied, re-read the existing file rather than
            // blanking the field on disk.
            let api_key = if req.api_key.is_none() && req.api_key_keep {
                let existing = state.config_dir.join("backends").join(format!("{name}.toml"));
                match n3ur0n_node::manifest::parse_backend_file(&existing) {
                    Ok(m) => match m.kind {
                        MfBackendKind::OpenAICompat(cfg) => cfg.api_key,
                        _ => String::new(),
                    },
                    Err(_) => String::new(),
                }
            } else {
                req.api_key.clone().unwrap_or_default()
            };
            let api_key = api_key.as_str();
            format!(
                r#"[manifest]
version = "0.1"

[backend]
name = "{name}"
kind = "openai_compat"

[openai_compat]
base_url      = "{base_url}"
default_model = "{default_model}"
api_key       = "{api_key}"
"#,
                name = name,
                base_url = toml_escape(&base_url),
                default_model = toml_escape(default_model),
                api_key = toml_escape(api_key),
            )
        }
        "mcp_server" => {
            let transport = req.transport.as_deref().unwrap_or("stdio");
            if !matches!(transport, "stdio" | "http_sse") {
                return settings_error(StatusCode::BAD_REQUEST, "transport must be stdio|http_sse");
            }
            let Some(command) = req.command.as_deref() else {
                return settings_error(StatusCode::BAD_REQUEST, "command required for mcp_server");
            };
            if command.trim().is_empty() {
                return settings_error(StatusCode::BAD_REQUEST, "command required for mcp_server");
            }
            let args_toml = if req.args.is_empty() {
                String::new()
            } else {
                format!(
                    "args = [{}]\n",
                    req.args.iter()
                        .map(|a| format!("\"{}\"", toml_escape(a)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let env_toml = if req.env.is_empty() {
                String::new()
            } else {
                let mut s = String::from("\n[mcp_server.env]\n");
                for (k, v) in &req.env {
                    s.push_str(&format!("{} = \"{}\"\n", k, toml_escape(v)));
                }
                s
            };
            format!(
                r#"[manifest]
version = "0.1"

[backend]
name = "{name}"
kind = "mcp_server"

[mcp_server]
transport = "{transport}"
command   = "{command}"
{args_toml}{env_toml}"#,
                name = name,
                transport = transport,
                command = toml_escape(command),
            )
        }
        "http_base" => {
            let Some(base_url) = req.base_url.as_deref() else {
                return settings_error(StatusCode::BAD_REQUEST, "base_url required for http_base");
            };
            let headers_toml = if req.headers.is_empty() {
                String::new()
            } else {
                let mut s = String::from("\n[http_base.headers]\n");
                for (k, v) in &req.headers {
                    s.push_str(&format!("\"{}\" = \"{}\"\n", k.replace('"', "\\\""), toml_escape(v)));
                }
                s
            };
            format!(
                r#"[manifest]
version = "0.1"

[backend]
name = "{name}"
kind = "http_base"

[http_base]
base_url = "{base_url}"
{headers_toml}"#,
                name = name,
                base_url = toml_escape(base_url),
            )
        }
        other => {
            return settings_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown kind `{other}` (expected openai_compat|mcp_server|http_base)"),
            );
        }
    };

    let backends_dir = state.config_dir.join("backends");
    if let Err(e) = std::fs::create_dir_all(&backends_dir) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let target = backends_dir.join(format!("{name}.toml"));
    let existed = target.exists();
    if let Err(e) = std::fs::write(&target, body) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let (backends_len, caps_len, reload_warning) =
        match state.node.reload_backends_from_manifest_dir() {
            Ok((b, c)) => (b, c, None),
            Err(e) => {
                tracing::warn!(error = %e, "backend reload after upsert failed");
                (0, 0, Some(e.to_string()))
            }
        };
    let (planner_reloaded, planner_reload_warning, planner_active) =
        match state.planner.as_ref() {
            Some(handle) => match crate::planner_config::hot_reload_planner_after_backend_change(
                &state.node,
                &state.config_dir,
                handle,
                &name,
            ) {
                Ok(Some(resolved)) => (
                    true,
                    None,
                    Some(json!({
                        "base_url": resolved.openai.base_url,
                        "model": resolved.model_hint,
                        "backend": resolved.backend_name,
                    })),
                ),
                Ok(None) => (false, None, None),
                Err(e) => {
                    tracing::warn!(error = %e, backend = %name, "planner reload after upsert failed");
                    (false, Some(e.to_string()), None)
                }
            },
            None => (false, None, None),
        };
    Json(json!({
        "ok": true,
        "name": name,
        "path": target.display().to_string(),
        "updated": existed,
        "backends_loaded": backends_len,
        "caps_loaded": caps_len,
        "reload_warning": reload_warning,
        "planner_reloaded": planner_reloaded,
        "planner_reload_warning": planner_reload_warning,
        "planner_active": planner_active,
    }))
    .into_response()
}

async fn get_backend(
    AxumState(state): AxumState<SettingsState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return settings_error(StatusCode::BAD_REQUEST, "invalid name");
    }
    let target = state.config_dir.join("backends").join(format!("{name}.toml"));
    if !target.exists() {
        return settings_error(StatusCode::NOT_FOUND, "backend manifest not found");
    }
    // Re-parse the file so we return a structured shape rather than raw
    // TOML; the UI prefill is the only caller and it wants typed fields.
    let parsed = match n3ur0n_node::manifest::parse_backend_file(&target) {
        Ok(m) => m,
        Err(e) => return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let body = match &parsed.kind {
        MfBackendKind::OpenAICompat(cfg) => json!({
            "name": parsed.name,
            "kind": "openai_compat",
            "base_url": cfg.base_url,
            "default_model": cfg.default_model,
            // api_key returned masked. Edit form lets the user re-enter
            // (or leave blank to preserve the existing value — handled
            // client-side).
            "has_api_key": !cfg.api_key.is_empty(),
        }),
        MfBackendKind::McpServer(cfg) => json!({
            "name": parsed.name,
            "kind": "mcp_server",
            "transport": format!("{:?}", cfg.transport).to_lowercase(),
            "command": cfg.command,
            "args": cfg.args,
            "env": cfg.env,
        }),
        MfBackendKind::HttpBase(cfg) => json!({
            "name": parsed.name,
            "kind": "http_base",
            "base_url": cfg.base_url,
            "headers": cfg.headers,
        }),
    };
    Json(body).into_response()
}

async fn delete_backend(
    AxumState(state): AxumState<SettingsState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return settings_error(StatusCode::BAD_REQUEST, "invalid name");
    }
    let target = state.config_dir.join("backends").join(format!("{name}.toml"));
    if !target.exists() {
        return settings_error(StatusCode::NOT_FOUND, "backend manifest not found");
    }
    if let Err(e) = std::fs::remove_file(&target) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let was_planner_backend = {
        let user = crate::planner_config::load_planner_user_config(&state.config_dir);
        user.backend.as_deref() == Some(name.as_str())
    };
    clear_planner_backend_if_removed(&state.config_dir, &name);

    let (backends_len, caps_len, reload_warning) =
        match state.node.reload_backends_from_manifest_dir() {
            Ok((b, c)) => (b, c, None),
            Err(e) => {
                tracing::warn!(error = %e, "backend reload after delete failed");
                (0, 0, Some(e.to_string()))
            }
        };
    let (planner_reloaded, planner_reload_warning) =
        if was_planner_backend {
            match state.planner.as_ref() {
                Some(handle) => {
                    let user = crate::planner_config::load_planner_user_config(&state.config_dir);
                    match crate::planner_config::hot_reload_planner_runtime(
                        &state.node,
                        &state.config_dir,
                        handle,
                        &user,
                    ) {
                        Ok(_) => (true, None),
                        Err(e) => {
                            tracing::warn!(error = %e, backend = %name, "planner reload after delete failed");
                            (false, Some(e.to_string()))
                        }
                    }
                }
                None => (false, None),
            }
        } else {
            (false, None)
        };
    Json(json!({
        "ok": true,
        "name": name,
        "backends_loaded": backends_len,
        "caps_loaded": caps_len,
        "reload_warning": reload_warning,
        "planner_reloaded": planner_reloaded,
        "planner_reload_warning": planner_reload_warning,
    }))
    .into_response()
}

fn clear_planner_backend_if_removed(config_dir: &std::path::Path, name: &str) {
    let mut user = crate::planner_config::load_planner_user_config(config_dir);
    if user.backend.as_deref() != Some(name) {
        return;
    }
    user.backend = None;
    if let Err(e) = crate::planner_config::save_planner_user_config(config_dir, &user) {
        tracing::warn!(error = %e, backend = %name, "failed to clear planner backend after delete");
    }
}

// ---------------------------------------------------------------------------
// Capability manifest CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpsertCapRequest {
    name: String,
    version: String,
    description: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    countries: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    lobe_ids: Vec<String>,
    #[serde(default)]
    disambiguation: Option<String>,
    #[serde(default)]
    output_semantic: Option<String>,
    schema_in: Value,
    schema_out: Value,
    examples: Vec<CapExampleReq>,
    binding: CapBindingReq,
}

fn default_mode() -> String { "free".into() }

#[derive(Debug, Deserialize)]
struct CapExampleReq {
    user_intent: String,
    args: Value,
    expected_output: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CapBindingReq {
    Prompt {
        backend: String,
        system_prompt: String,
        #[serde(default)]
        user_template: Option<String>,
        #[serde(default)]
        parameters: std::collections::HashMap<String, Value>,
        #[serde(default = "default_parser")]
        output_parser: String,
        #[serde(default)]
        model: Option<String>,
    },
    Mcp {
        backend: String,
        tool_name: String,
        #[serde(default)]
        arg_mapping: std::collections::HashMap<String, Value>,
        #[serde(default)]
        result_mapping: std::collections::HashMap<String, Value>,
    },
    Http {
        backend: String,
        url_template: String,
        method: String,
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        #[serde(default)]
        body_template: Option<Value>,
        #[serde(default)]
        response_path: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

fn default_parser() -> String { "text".into() }

async fn list_cap_manifests(AxumState(state): AxumState<SettingsState>) -> impl IntoResponse {
    use n3ur0n_node::manifest::load_cap_dir;
    let dir = state.config_dir.join("caps");
    let mut out: Vec<Value> = Vec::new();
    for result in load_cap_dir(&dir) {
        match result {
            Ok(m) => out.push(json!({
                "name": m.descriptor.name,
                "version": m.descriptor.version,
                "description": m.descriptor.description,
                "binding_type": binding_type_str(&m.binding),
                "binding_backend": m.binding.backend(),
            })),
            Err(e) => out.push(json!({"name": null, "error": e.to_string()})),
        }
    }
    Json(json!({ "caps": out, "dir": dir.display().to_string() })).into_response()
}

fn binding_type_str(spec: &n3ur0n_node::manifest::BindingSpec) -> &'static str {
    use n3ur0n_node::manifest::BindingSpec as BS;
    match spec {
        BS::Prompt { .. } => "prompt",
        BS::Mcp { .. } => "mcp",
        BS::Http { .. } => "http",
    }
}

async fn get_cap_manifest(
    AxumState(state): AxumState<SettingsState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    let target = state.config_dir.join("caps").join(format!("{name}.toml"));
    if !target.exists() {
        return settings_error(StatusCode::NOT_FOUND, "cap manifest not found");
    }
    match std::fs::read_to_string(&target) {
        Ok(raw) => Json(json!({"name": name, "toml": raw})).into_response(),
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn delete_cap_manifest(
    AxumState(state): AxumState<SettingsState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return settings_error(StatusCode::BAD_REQUEST, "invalid name");
    }
    let target = state.config_dir.join("caps").join(format!("{name}.toml"));
    if !target.exists() {
        return settings_error(StatusCode::NOT_FOUND, "cap manifest not found");
    }
    if let Err(e) = std::fs::remove_file(&target) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let registered = match state.node.reload_caps_from_manifest_dir() {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "cap reload after delete failed");
            return settings_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("delete ok but reload failed: {e}"),
            );
        }
    };
    Json(json!({"ok": true, "name": name, "registered": registered})).into_response()
}

async fn upsert_cap_manifest(
    AxumState(state): AxumState<SettingsState>,
    Json(req): Json<UpsertCapRequest>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "name must be non-empty and match [a-zA-Z0-9_-]",
        );
    }
    if semver::Version::parse(&req.version).is_err() {
        return settings_error(StatusCode::BAD_REQUEST, "version must be valid semver (e.g. 0.1.0)");
    }
    if req.description.trim().is_empty() {
        return settings_error(StatusCode::BAD_REQUEST, "description is required");
    }
    if req.examples.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "at least one example is required (the planner refuses caps with no examples)",
        );
    }

    let toml_body = match build_cap_toml(name, &req) {
        Ok(s) => s,
        Err(e) => return settings_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };

    let caps_dir = state.config_dir.join("caps");
    if let Err(e) = std::fs::create_dir_all(&caps_dir) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let target = caps_dir.join(format!("{name}.toml"));
    if let Err(e) = std::fs::write(&target, toml_body) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let registered = match state.node.reload_caps_from_manifest_dir() {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "cap reload after upsert failed");
            return settings_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("saved but reload failed: {e}"),
            );
        }
    };
    Json(json!({
        "ok": true,
        "name": name,
        "path": target.display().to_string(),
        "registered": registered,
    }))
    .into_response()
}

fn build_cap_toml(name: &str, req: &UpsertCapRequest) -> Result<String, String> {
    use std::fmt::Write;

    fn json_to_toml_value(v: &Value) -> String {
        match v {
            Value::Null => "\"\"".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => format!(
                "\"{}\"",
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t"),
            ),
            Value::Array(arr) => format!(
                "[{}]",
                arr.iter().map(json_to_toml_value).collect::<Vec<_>>().join(", ")
            ),
            Value::Object(obj) => format!(
                "{{ {} }}",
                obj.iter()
                    .map(|(k, v)| format!("{} = {}", toml_key(k), json_to_toml_value(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    fn toml_key(k: &str) -> String {
        if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            k.to_string()
        } else {
            format!("\"{}\"", k.replace('"', "\\\""))
        }
    }
    fn toml_str(s: &str) -> String {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t"),
        )
    }
    fn toml_multiline(s: &str) -> String {
        if s.contains('\n') {
            format!("\"\"\"\n{}\n\"\"\"", s.replace("\"\"\"", "\\\"\\\"\\\""))
        } else {
            toml_str(s)
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "[manifest]");
    let _ = writeln!(out, "version = \"0.1\"\n");
    let _ = writeln!(out, "[descriptor]");
    let _ = writeln!(out, "name = {}", toml_str(name));
    let _ = writeln!(out, "version = {}", toml_str(&req.version));
    let _ = writeln!(out, "description = {}", toml_str(&req.description));
    let _ = writeln!(out, "mode = {}", toml_str(&req.mode));
    if !req.tags.is_empty() {
        let _ = writeln!(
            out,
            "tags = [{}]",
            req.tags.iter().map(|t| toml_str(t)).collect::<Vec<_>>().join(", ")
        );
    }
    if !req.lobe_ids.is_empty() {
        let _ = writeln!(
            out,
            "lobe_ids = [{}]",
            req.lobe_ids.iter().map(|t| toml_str(t)).collect::<Vec<_>>().join(", ")
        );
    }
    if !req.languages.is_empty() {
        let _ = writeln!(
            out,
            "languages = [{}]",
            req.languages.iter().map(|t| toml_str(t)).collect::<Vec<_>>().join(", ")
        );
    }
    if !req.countries.is_empty() {
        let _ = writeln!(
            out,
            "countries = [{}]",
            req.countries.iter().map(|t| toml_str(t)).collect::<Vec<_>>().join(", ")
        );
    }
    if let Some(d) = &req.disambiguation {
        if !d.trim().is_empty() {
            let _ = writeln!(out, "disambiguation = {}", toml_multiline(d));
        }
    }
    if let Some(o) = &req.output_semantic {
        if !o.trim().is_empty() {
            let _ = writeln!(out, "output_semantic = {}", toml_multiline(o));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "[descriptor.schema_in]");
    if let Value::Object(map) = &req.schema_in {
        for (k, v) in map {
            let _ = writeln!(out, "{} = {}", toml_key(k), json_to_toml_value(v));
        }
    } else {
        return Err("schema_in must be a JSON object".into());
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "[descriptor.schema_out]");
    if let Value::Object(map) = &req.schema_out {
        for (k, v) in map {
            let _ = writeln!(out, "{} = {}", toml_key(k), json_to_toml_value(v));
        }
    } else {
        return Err("schema_out must be a JSON object".into());
    }

    for ex in &req.examples {
        let _ = writeln!(out, "\n[[descriptor.examples]]");
        let _ = writeln!(out, "user_intent = {}", toml_str(&ex.user_intent));
        let _ = writeln!(out, "args = {}", json_to_toml_value(&ex.args));
        let _ = writeln!(out, "expected_output = {}", json_to_toml_value(&ex.expected_output));
    }

    match &req.binding {
        CapBindingReq::Prompt {
            backend,
            system_prompt,
            user_template,
            parameters,
            output_parser,
            model,
        } => {
            let _ = writeln!(out, "\n[binding]");
            let _ = writeln!(out, "type = \"prompt\"");
            let _ = writeln!(out, "backend = {}", toml_str(backend));
            let _ = writeln!(out, "\n[binding.prompt]");
            let _ = writeln!(out, "system_prompt = {}", toml_multiline(system_prompt));
            if let Some(t) = user_template {
                if !t.trim().is_empty() {
                    let _ = writeln!(out, "user_template = {}", toml_multiline(t));
                }
            }
            if !parameters.is_empty() {
                let pairs: Vec<String> = parameters
                    .iter()
                    .map(|(k, v)| format!("{} = {}", toml_key(k), json_to_toml_value(v)))
                    .collect();
                let _ = writeln!(out, "parameters = {{ {} }}", pairs.join(", "));
            }
            let _ = writeln!(out, "output_parser = {}", toml_str(output_parser));
            if let Some(m) = model {
                if !m.trim().is_empty() {
                    let _ = writeln!(out, "model = {}", toml_str(m));
                }
            }
        }
        CapBindingReq::Mcp {
            backend,
            tool_name,
            arg_mapping,
            result_mapping,
        } => {
            if tool_name.trim().is_empty() {
                return Err("binding.mcp.tool_name is required".into());
            }
            let _ = writeln!(out, "\n[binding]");
            let _ = writeln!(out, "type = \"mcp\"");
            let _ = writeln!(out, "backend = {}", toml_str(backend));
            let _ = writeln!(out, "\n[binding.mcp]");
            let _ = writeln!(out, "tool_name = {}", toml_str(tool_name));
            if !arg_mapping.is_empty() {
                let pairs: Vec<String> = arg_mapping
                    .iter()
                    .map(|(k, v)| format!("{} = {}", toml_key(k), json_to_toml_value(v)))
                    .collect();
                let _ = writeln!(out, "arg_mapping = {{ {} }}", pairs.join(", "));
            }
            if !result_mapping.is_empty() {
                let pairs: Vec<String> = result_mapping
                    .iter()
                    .map(|(k, v)| format!("{} = {}", toml_key(k), json_to_toml_value(v)))
                    .collect();
                let _ = writeln!(out, "result_mapping = {{ {} }}", pairs.join(", "));
            }
        }
        CapBindingReq::Http {
            backend,
            url_template,
            method,
            headers,
            body_template,
            response_path,
            timeout_ms,
        } => {
            if url_template.trim().is_empty() {
                return Err("binding.http.url_template is required".into());
            }
            let m = method.to_ascii_uppercase();
            if !matches!(m.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
                return Err(format!("binding.http.method `{method}` invalid (GET|POST|PUT|DELETE)"));
            }
            let _ = writeln!(out, "\n[binding]");
            let _ = writeln!(out, "type = \"http\"");
            let _ = writeln!(out, "backend = {}", toml_str(backend));
            let _ = writeln!(out, "\n[binding.http]");
            let _ = writeln!(out, "url_template = {}", toml_str(url_template));
            let _ = writeln!(out, "method = {}", toml_str(&m));
            if let Some(t) = timeout_ms {
                let _ = writeln!(out, "timeout_ms = {t}");
            }
            if let Some(p) = response_path {
                if !p.trim().is_empty() {
                    let _ = writeln!(out, "response_path = {}", toml_str(p));
                }
            }
            if let Some(b) = body_template {
                let _ = writeln!(out, "body_template = {}", json_to_toml_value(b));
            }
            if !headers.is_empty() {
                let _ = writeln!(out, "\n[binding.http.headers]");
                for (k, v) in headers {
                    let _ = writeln!(out, "{} = {}", toml_key(k), toml_str(v));
                }
            }
        }
    }
    Ok(out)
}
