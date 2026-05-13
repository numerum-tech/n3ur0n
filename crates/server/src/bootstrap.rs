//! Wiring helpers: paths, node construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use n3ur0n_adapters::{
    Backend,
    echo::EchoBackend,
    openai::{OpenAIBackend, OpenAIConfig},
    utility::UtilityBackend,
};
use n3ur0n_core::Keypair;
use n3ur0n_node::backends_registry::BackendsRegistry;
use n3ur0n_node::bindings::{build_binding, Binding};
use n3ur0n_node::client as peer_client;
use n3ur0n_node::manifest::{load_backend_dir, load_cap_dir};
use n3ur0n_node::planner::compiler::{
    CascadingCompiler, LocalLLMCompiler, PlanCompiler, RemotePlanCompiler,
};
use n3ur0n_node::planner::plan_exec::default_compile_system_prompt;
use n3ur0n_node::planner::{PlanExecPlanner, Planner};
use n3ur0n_node::runtime::{NodeRuntime, RuntimeConfig};
use n3ur0n_node::{CapabilityRegistry, IdentityFile, Node, NodeConfig, identity_file};

pub fn default_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("n3ur0n")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/n3ur0n")
    } else {
        PathBuf::from(".n3ur0n")
    }
}

pub fn db_path(dir: &Path) -> PathBuf {
    dir.join("n3ur0n.sqlite")
}

pub fn keys_path(dir: &Path) -> PathBuf {
    identity_file::default_path(dir)
}

/// Build a fully-wired [`Node`] from a config directory: load identity, open
/// db, build a backend from the runtime selector, populate registry.
pub async fn load_node(
    config_dir: &Path,
    endpoint: Option<String>,
    bootstrap_peers: Vec<String>,
    backend_kind: BackendKind,
) -> Result<Node> {
    let kp = IdentityFile::load(&keys_path(config_dir))
        .with_context(|| format!("loading identity from {}", keys_path(config_dir).display()))?;
    let db = n3ur0n_storage::open(db_path(config_dir))
        .with_context(|| format!("opening db at {}", db_path(config_dir).display()))?;

    let cfg = NodeConfig {
        endpoint,
        alias: None,
        bootstrap_peers,
        ..Default::default()
    };

    // Two paths:
    //   - manifest mode: scan <config_dir>/{backends,caps}/ and build a
    //     CapabilityRegistry whose entries each carry a binding. The
    //     single legacy backend on `Node` is set to an inert EchoBackend
    //     and never invoked (handler dispatches via the binding).
    //   - legacy mode: pick one compile-time backend, call describe(),
    //     build a binding-less CapabilityRegistry.
    match backend_kind {
        BackendKind::Manifest { dir } => {
            let (registry, backends) = load_manifest_registry(&dir).await?;
            let inert: Arc<dyn Backend> = Arc::new(EchoBackend);
            // Attach the BackendsRegistry + dir so the node can
            // hot-reload caps later without a restart.
            Ok(Node::new(kp, db, inert, registry, cfg)
                .with_manifest_runtime(Arc::new(backends), dir))
        }
        other => {
            let backend: Arc<dyn Backend> = build_backend(other)?;
            let decls = backend.describe().await?;
            let registry = CapabilityRegistry::from_decls(decls);
            Ok(Node::new(kp, db, backend, registry, cfg))
        }
    }
}

/// Scan `<manifest_dir>/backends/` and `<manifest_dir>/caps/`, build a
/// `BackendsRegistry`, materialise one `Binding` per cap and register it.
/// Malformed manifests are logged + skipped so one broken file never
/// takes the whole node down. Returns `(caps_registry, backends_registry)`
/// so the caller can attach both to the Node for cap hot-reload.
async fn load_manifest_registry(
    dir: &Path,
) -> Result<(CapabilityRegistry, BackendsRegistry)> {
    let backends_dir = dir.join("backends");
    let caps_dir = dir.join("caps");

    let mut backend_manifests = Vec::new();
    for result in load_backend_dir(&backends_dir) {
        match result {
            Ok(m) => backend_manifests.push(m),
            Err(e) => tracing::warn!(error = %e, "skipping malformed backend manifest"),
        }
    }
    let backends = BackendsRegistry::from_manifests(backend_manifests)
        .map_err(|e| anyhow::anyhow!("backends registry: {e}"))?;
    tracing::info!(
        backends_dir = %backends_dir.display(),
        loaded = backends.len(),
        "manifest mode: backends loaded"
    );

    let mut entries: Vec<(n3ur0n_core::CapabilityDecl, Arc<dyn Binding>)> = Vec::new();
    for result in load_cap_dir(&caps_dir) {
        let cap = match result {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "skipping malformed cap manifest");
                continue;
            }
        };
        let backend_name = cap.binding.backend().to_string();
        let backend_instance = match backends.get(&backend_name) {
            Some(b) => b,
            None => {
                tracing::warn!(
                    cap = %cap.descriptor.name,
                    backend = %backend_name,
                    "cap references unknown backend; skipping"
                );
                continue;
            }
        };
        let binding = match build_binding(&cap.binding, backend_instance) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    cap = %cap.descriptor.name,
                    error = %e,
                    "binding construction failed; skipping cap"
                );
                continue;
            }
        };
        entries.push((cap.descriptor, binding));
    }
    tracing::info!(
        caps_dir = %caps_dir.display(),
        loaded = entries.len(),
        "manifest mode: capabilities registered"
    );
    Ok((CapabilityRegistry::from_entries(entries), backends))
}

/// Backend selector resolved from CLI flags / env at startup.
#[derive(Debug, Clone)]
pub enum BackendKind {
    /// Identity-style adapter; useful for cluster smoke and tests.
    Echo,
    /// Multi-cap utility backend: time, random_int, reverse, string_length.
    Utility,
    /// OpenAI-compatible chat endpoint (Ollama, llama.cpp, vLLM, OpenAI...).
    OpenAI(OpenAIConfig),
    /// v0.3: capabilities loaded from `<dir>/{backends,caps}/*.toml` at
    /// startup. Each cap carries its own binding (prompt / http / mcp).
    /// The single compile-time backend slot is filled with an inert
    /// placeholder (EchoBackend) and never invoked.
    Manifest { dir: PathBuf },
}

impl Default for BackendKind {
    fn default() -> Self {
        Self::Echo
    }
}

fn build_backend(kind: BackendKind) -> Result<Arc<dyn Backend>> {
    match kind {
        BackendKind::Echo => Ok(Arc::new(EchoBackend)),
        BackendKind::Utility => Ok(Arc::new(UtilityBackend)),
        BackendKind::OpenAI(cfg) => {
            let backend = OpenAIBackend::new(cfg)
                .map_err(|e| anyhow::anyhow!("openai backend init: {e}"))?;
            Ok(Arc::new(backend))
        }
        BackendKind::Manifest { .. } => {
            anyhow::bail!("build_backend called with Manifest kind; should be handled in load_node")
        }
    }
}

/// Planner selector.
///
/// v0.2 ships only `PlanExec`. The legacy ReAct planner (`LLMPlanner`) was
/// removed because its multi-turn tool-calling loop is unreliable on small
/// (≤8B) models. See branch `archive/llm-react` if it ever needs to come
/// back.
#[derive(Debug, Clone)]
pub enum PlannerKind {
    /// Plan-then-execute: one LLM call compiles a typed plan, a deterministic
    /// executor walks it, a final LLM call composes the user-facing reply.
    PlanExec {
        backend: OpenAIConfig,
        model_hint: Option<String>,
    },
}

/// Build a `NodeRuntime` (Node + Planner + concurrency primitives) from a
/// resolved `PlannerKind`.
///
/// If `N3UR0N_PLANNER_REMOTE_FALLBACK` is set to a peer endpoint (e.g.
/// `http://node-c:4242`), the planner wraps its local compiler in a
/// `CascadingCompiler` that escalates to that peer's `plan` capability
/// when the local compile produces a low-confidence plan. The escalation
/// threshold is set by `N3UR0N_PLANNER_CONFIDENCE_THRESHOLD` (default
/// 0.5).
pub fn build_runtime(
    node: Node,
    kind: PlannerKind,
    runtime_config: RuntimeConfig,
) -> Result<NodeRuntime> {
    let planner: Arc<dyn Planner> = match kind {
        PlannerKind::PlanExec { backend, model_hint } => {
            let llm: Arc<dyn Backend> = Arc::new(
                OpenAIBackend::new(backend.clone())
                    .map_err(|e| anyhow::anyhow!("planner llm init: {e}"))?,
            );
            let chosen_model = model_hint.unwrap_or(backend.default_model);

            let local = Arc::new(LocalLLMCompiler {
                llm_backend: llm.clone(),
                model_hint: Some(chosen_model.clone()),
                system_prompt: Arc::new(default_compile_system_prompt),
            });

            let compiler: Arc<dyn PlanCompiler> =
                match std::env::var("N3UR0N_PLANNER_REMOTE_FALLBACK").ok() {
                    Some(endpoint) if !endpoint.trim().is_empty() => {
                        let threshold = std::env::var("N3UR0N_PLANNER_CONFIDENCE_THRESHOLD")
                            .ok()
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(0.5);
                        let remote = Arc::new(RemotePlanCompiler {
                            http: peer_client::http_client(),
                            keypair: node.keypair().clone(),
                            endpoint: endpoint.trim().to_string(),
                            sender_endpoint: node.config().endpoint.clone(),
                        });
                        tracing::info!(
                            remote_endpoint = %remote.endpoint,
                            threshold,
                            "planner: cascading compiler enabled"
                        );
                        Arc::new(CascadingCompiler {
                            primary: local,
                            fallback: Some(remote),
                            threshold,
                        })
                    }
                    _ => local,
                };

            Arc::new(PlanExecPlanner::with_compiler(
                compiler,
                llm,
                Some(chosen_model),
            ))
        }
    };
    Ok(NodeRuntime::new(node, planner, runtime_config))
}

/// Generate a fresh identity, persist it, and return the underlying keypair.
pub fn create_identity(config_dir: &Path) -> Result<Keypair> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let kp = Keypair::generate();
    IdentityFile::from_keypair(&kp).save(&keys_path(config_dir))?;
    // Initialise the database so first `serve` does not race.
    let _ = n3ur0n_storage::open(db_path(config_dir))?;
    Ok(kp)
}
