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
use n3ur0n_node::planner::{LLMPlanner, Planner};
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

    let backend: Arc<dyn Backend> = build_backend(backend_kind)?;
    let decls = backend.describe().await?;
    let registry = CapabilityRegistry::from_decls(decls);

    let cfg = NodeConfig {
        endpoint,
        alias: None,
        bootstrap_peers,
        ..Default::default()
    };

    Ok(Node::new(kp, db, backend, registry, cfg))
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
    }
}

/// Planner selector. v0.1 only ships LLM-based planner.
#[derive(Debug, Clone)]
pub enum PlannerKind {
    /// LLM-driven, native tool-calling.
    Llm {
        /// Backend used for the planning conversation (an OpenAI-compatible
        /// endpoint).
        backend: OpenAIConfig,
        /// Optional default model for the planner; falls back to the
        /// `OpenAIConfig.default_model` if `None`.
        model_hint: Option<String>,
    },
}

/// Build a `NodeRuntime` (Node + Planner + concurrency primitives) from a
/// resolved `PlannerKind`.
pub fn build_runtime(
    node: Node,
    kind: PlannerKind,
    runtime_config: RuntimeConfig,
) -> Result<NodeRuntime> {
    let planner: Arc<dyn Planner> = match kind {
        PlannerKind::Llm { backend, model_hint } => {
            let llm: Arc<dyn Backend> = Arc::new(
                OpenAIBackend::new(backend.clone())
                    .map_err(|e| anyhow::anyhow!("planner llm init: {e}"))?,
            );
            let chosen_model = model_hint.unwrap_or(backend.default_model);
            Arc::new(LLMPlanner::new(llm, Some(chosen_model)))
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
