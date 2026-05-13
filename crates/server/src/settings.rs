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
//! `<config>/caps/<name>.toml`. Capability CRUD also triggers a live
//! `Node::reload_caps_from_manifest_dir()` so the in-memory registry
//! reflects the change without restart. Backend CRUD still requires a
//! restart (backend hot-reload is deferred).
//!
//! Lifted from the desktop shell so the headless server exposes the
//! same Settings surface to the embedded web UI.

use std::path::PathBuf;

use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::{Json, Router};
use n3ur0n_node::manifest::{load_backend_dir, BackendKind as MfBackendKind};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

#[derive(Clone, Debug)]
pub struct SettingsState {
    pub config_dir: PathBuf,
    pub node: n3ur0n_node::Node,
}

pub fn router(config_dir: PathBuf, node: n3ur0n_node::Node) -> Router {
    let state = SettingsState { config_dir, node };
    Router::new()
        .route("/api/v0/backends", get(list_backends).post(create_backend))
        .route("/api/v0/backends/:name", delete(delete_backend))
        .route(
            "/api/v0/caps/manifests",
            get(list_cap_manifests).post(upsert_cap_manifest),
        )
        .route(
            "/api/v0/caps/manifests/:name",
            get(get_cap_manifest).delete(delete_cap_manifest),
        )
        .with_state(state)
}

fn settings_error(status: StatusCode, message: &str) -> axum::response::Response {
    (status, Json(json!({"error": message}))).into_response()
}

// ---------------------------------------------------------------------------
// Backend CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateBackendRequest {
    name: String,
    kind: String,
    base_url: Option<String>,
    default_model: Option<String>,
    api_key: Option<String>,
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
    if req.kind != "openai_compat" {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "v0.3.0 Settings UI only supports kind=openai_compat; edit TOML directly for mcp_server/http_base",
        );
    }
    let Some(base_url) = req.base_url else {
        return settings_error(StatusCode::BAD_REQUEST, "base_url required for openai_compat");
    };
    let Some(default_model) = req.default_model else {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "default_model required for openai_compat",
        );
    };
    let api_key = req.api_key.unwrap_or_default();

    let backends_dir = state.config_dir.join("backends");
    if let Err(e) = std::fs::create_dir_all(&backends_dir) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let target = backends_dir.join(format!("{name}.toml"));
    let body = format!(
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
        base_url = base_url.replace('"', "\\\""),
        default_model = default_model.replace('"', "\\\""),
        api_key = api_key.replace('"', "\\\""),
    );
    if let Err(e) = std::fs::write(&target, body) {
        return settings_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({
        "ok": true,
        "name": name,
        "path": target.display().to_string(),
        "requires_restart": true,
    }))
    .into_response()
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
    Json(json!({
        "ok": true,
        "name": name,
        "requires_restart": true,
    }))
    .into_response()
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
            Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
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
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
    }
    Ok(out)
}
