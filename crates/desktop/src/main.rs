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
use n3ur0n_adapters::openai::OpenAIConfig;
use n3ur0n_node::manifest::{load_backend_dir, BackendKind as MfBackendKind};
use n3ur0n_node::runtime::{NodeRuntime, RuntimeConfig};
use n3ur0n_server::bootstrap::{self, BackendKind, PlannerKind};
use n3ur0n_server::http;
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

    let app = http::app(node, runtime);
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
