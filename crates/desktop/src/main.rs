//! N3UR0N desktop shell (Tauri 2).
//!
//! Wraps an in-process `n3ur0n-server` listening on `127.0.0.1:<random>` and
//! points the Tauri window at `http://127.0.0.1:<port>/ui/`. The
//! capability stack — manifest registry, planner, bindings, peer client —
//! is the same code path as the headless `n3ur0n serve` binary. Only the
//! shell + lifecycle differ:
//!
//! - **Listener is loopback-only.** This is a *consumer* node by default
//!   (no public endpoint). Peers cannot reach it; the user can still
//!   call out to remote peers.
//! - **Identity + storage** live in the OS-standard app config dir
//!   (`~/Library/Application Support/n3ur0n` on macOS, `%APPDATA%\n3ur0n`
//!   on Windows, `~/.config/n3ur0n` on Linux).
//! - **Ollama auto-detect** on first launch: if `http://localhost:11434`
//!   responds to `/v1/models`, a default `local_ollama` backend
//!   manifest is scaffolded so the planner has something to use.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::{Json, Router};
use n3ur0n_adapters::openai::OpenAIConfig;
use n3ur0n_node::manifest::{load_backend_dir, BackendKind as MfBackendKind};
use n3ur0n_node::runtime::{NodeRuntime, RuntimeConfig};
use n3ur0n_server::bootstrap::{self, BackendKind, PlannerKind};
use n3ur0n_server::http;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tracing::{info, warn};

const TUI_PROBE_OLLAMA_URL: &str = "http://localhost:11434/v1/models";
const OLLAMA_BACKEND_NAME: &str = "local_ollama";
const OLLAMA_DEFAULT_MODEL: &str = "llama3.1:8b";

fn app_config_dir() -> PathBuf {
    // dirs::config_dir() resolves to platform-standard locations.
    dirs::config_dir()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd available"))
        .join("n3ur0n")
}

/// Detect a local Ollama server. Returns `true` if `/v1/models` answers
/// 200 within a short timeout. Used at first launch to seed a useful
/// default backend.
async fn detect_ollama() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    matches!(
        client.get(TUI_PROBE_OLLAMA_URL).send().await,
        Ok(r) if r.status().is_success()
    )
}

/// Write a default `local_ollama` backend manifest the first time the app
/// boots (only when none exists and Ollama is reachable). Idempotent.
async fn maybe_scaffold_ollama_backend(config_dir: &PathBuf) -> Result<()> {
    let backends_dir = config_dir.join("backends");
    let caps_dir = config_dir.join("caps");
    std::fs::create_dir_all(&backends_dir)?;
    std::fs::create_dir_all(&caps_dir)?;

    let target = backends_dir.join(format!("{OLLAMA_BACKEND_NAME}.toml"));
    if target.exists() {
        return Ok(());
    }
    if !detect_ollama().await {
        info!(
            url = TUI_PROBE_OLLAMA_URL,
            "no local Ollama detected; first-launch backend scaffold skipped"
        );
        return Ok(());
    }
    let body = format!(
        r#"# Auto-scaffolded on first launch when a local Ollama server was
# detected at {url}. Edit freely; this file is local-only and not shared.
[manifest]
version = "0.1"

[backend]
name = "{name}"
kind = "openai_compat"

[openai_compat]
base_url      = "http://localhost:11434"
default_model = "{model}"
api_key       = ""
"#,
        url = TUI_PROBE_OLLAMA_URL,
        name = OLLAMA_BACKEND_NAME,
        model = OLLAMA_DEFAULT_MODEL,
    );
    std::fs::write(&target, body)
        .with_context(|| format!("writing default backend manifest to {target:?}"))?;
    info!(path = %target.display(), "scaffolded default Ollama backend manifest");
    Ok(())
}

/// Scan `<config>/backends/` and return the first `openai_compat` config
/// found, materialised as an `OpenAIConfig`. Used to auto-wire the
/// planner runtime so the chat tab works out of the box when an LLM
/// endpoint is reachable.
fn pick_planner_backend(backends_dir: &PathBuf) -> Option<OpenAIConfig> {
    if !backends_dir.exists() {
        return None;
    }
    for result in load_backend_dir(backends_dir) {
        let Ok(manifest) = result else { continue };
        if let MfBackendKind::OpenAICompat(cfg) = manifest.kind {
            return Some(OpenAIConfig {
                base_url: cfg.base_url,
                default_model: cfg.default_model,
                api_key: if cfg.api_key.is_empty() {
                    None
                } else {
                    Some(cfg.api_key)
                },
                description: None,
            });
        }
    }
    None
}

async fn start_server() -> Result<u16> {
    let config_dir = app_config_dir();
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("creating app config dir {}", config_dir.display()))?;

    // Generate identity on first launch.
    let keys_path = bootstrap::keys_path(&config_dir);
    if !keys_path.exists() {
        info!(path = %keys_path.display(), "first launch — generating identity");
        bootstrap::create_identity(&config_dir).context("creating identity")?;
    }

    maybe_scaffold_ollama_backend(&config_dir).await.ok();

    // Consumer profile: load_node in manifest mode pointed at config_dir
    // (which now contains backends/ + caps/). No public endpoint, no
    // bootstrap peers by default.
    let node = bootstrap::load_node(
        &config_dir,
        None, // endpoint = None: consumer, no public listener
        Vec::new(),
        BackendKind::Manifest {
            dir: config_dir.clone(),
        },
    )
    .await
    .context("loading node")?;

    // Pick a free local port.
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .context("binding loopback socket")?;
    let port = listener.local_addr()?.port();

    let runtime_config = RuntimeConfig::default();
    // Auto-wire the planner runtime when the user has at least one
    // `openai_compat` backend declared. We pick the FIRST such backend by
    // load order (deterministic per-filename sort) — heuristic, but it
    // means "Ollama auto-detected on first launch" → working chat with
    // zero further config. When no openai_compat backend exists, the
    // node still runs but the conversation routes return 503.
    let runtime: Option<Arc<NodeRuntime>> =
        match pick_planner_backend(&config_dir.join("backends")) {
            Some(cfg) => {
                let chosen_model = Some(cfg.default_model.clone());
                match bootstrap::build_runtime(
                    node.clone(),
                    PlannerKind::PlanExec {
                        backend: cfg,
                        model_hint: chosen_model,
                    },
                    runtime_config,
                ) {
                    Ok(rt) => {
                        info!("planner runtime wired");
                        Some(Arc::new(rt))
                    }
                    Err(e) => {
                        warn!(error = %e, "planner runtime failed to init; chat tab will return 503");
                        None
                    }
                }
            }
            None => {
                warn!("no openai_compat backend found in backends/; chat tab will return 503 until one is added");
                None
            }
        };

    // The headless server's router + our desktop-specific Settings
    // routes merged on the same listener. Settings endpoints CRUD the
    // manifest files on disk so the UI can configure backends without
    // shell access.
    let settings_state = SettingsState {
        config_dir: config_dir.clone(),
    };
    let settings_router = Router::new()
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
        .with_state(settings_state);

    let app = http::app(node, runtime).merge(settings_router);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "embedded server stopped");
        }
    });

    info!(port, "n3ur0n desktop server listening on 127.0.0.1");
    Ok(port)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,n3ur0n=debug".into()),
        )
        .init();

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let port = runtime
        .block_on(start_server())
        .expect("starting embedded server");
    let url = format!("http://127.0.0.1:{port}/ui/");

    // Keep the tokio runtime alive for the lifetime of the app — Tauri's
    // event loop owns the main thread, so we leak the handle on purpose.
    std::mem::forget(runtime);

    tauri::Builder::default()
        .setup(move |app| {
            let win = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url.parse().expect("valid url")),
            )
            .title("N3UR0N")
            .inner_size(1280.0, 800.0)
            .min_inner_size(900.0, 600.0)
            .build()?;
            // Best-effort focus.
            let _ = win.set_focus();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running n3ur0n desktop");
}

// ---------------------------------------------------------------------------
// Settings API — desktop-only routes for manifest CRUD over the local fs.
// ---------------------------------------------------------------------------
//
// Headless deployments use the file system directly (or a config-management
// tool). The desktop shell exposes a small REST surface so the bundled UI
// can offer a Settings tab without shell access.
//
// Endpoints (all bound to 127.0.0.1 — never accept remote connections):
//
//   GET    /api/v0/backends           — list parsed backend manifests
//   POST   /api/v0/backends           — create/overwrite a backend.toml
//   DELETE /api/v0/backends/:name     — remove a backend.toml
//
// CRUD operations write directly to `<config>/backends/<name>.toml`. A
// background watcher (Phase 4 of the manifest plan) will pick up the
// change and re-load the registry; until then the desktop app must be
// restarted for changes to take effect (we expose a `requires_restart`
// flag in responses so the UI can warn the user).

#[derive(Clone)]
struct SettingsState {
    config_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct CreateBackendRequest {
    name: String,
    kind: String,
    base_url: Option<String>,
    default_model: Option<String>,
    api_key: Option<String>,
}

fn settings_error(status: StatusCode, message: &str) -> axum::response::Response {
    (status, Json(json!({"error": message}))).into_response()
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
//
// The Skills tab needs a richer surface than `/api/v0/caps` (which is the
// runtime-registry view, stripped of binding details). These endpoints
// CRUD the raw `caps/*.toml` files: list all manifests, read one's raw
// TOML, write a new one, delete one. The composer form posts a JSON body
// describing the cap; the server serialises it to TOML on disk.

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
    Json(json!({"ok": true, "name": name, "requires_restart": true})).into_response()
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

    // Build the TOML body explicitly: toml::to_string preserves field
    // ordering oddly for inline tables — manual emission keeps the file
    // readable and matches the spec format.
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
    Json(json!({
        "ok": true,
        "name": name,
        "path": target.display().to_string(),
        "requires_restart": true,
    }))
    .into_response()
}

/// Render the JSON-shaped request as a cap.toml that the loader will
/// accept. Done manually (vs `toml::to_string`) so the output stays
/// readable and ordered like the hand-written examples in the spec.
fn build_cap_toml(name: &str, req: &UpsertCapRequest) -> Result<String, String> {
    use std::fmt::Write;

    fn json_to_toml_value(v: &Value) -> String {
        // Re-emit a JSON value as a TOML inline expression. For our use
        // case (schema fragments, args, expected_output) inline form is
        // the cleanest match.
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
        // Triple-quoted basic string for prompts with newlines.
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
