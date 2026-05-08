//! Wiring helpers: paths, node construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use n3ur0n_adapters::{Backend, echo::EchoBackend};
use n3ur0n_core::Keypair;
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
/// db, construct the default backend (echo for v0.1), build registry.
pub async fn load_node(config_dir: &Path, endpoint: Option<String>) -> Result<Node> {
    let kp = IdentityFile::load(&keys_path(config_dir))
        .with_context(|| format!("loading identity from {}", keys_path(config_dir).display()))?;
    let db = n3ur0n_storage::open(db_path(config_dir))
        .with_context(|| format!("opening db at {}", db_path(config_dir).display()))?;

    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let decls = backend.describe().await?;
    let registry = CapabilityRegistry::from_decls(decls);

    let cfg = NodeConfig {
        endpoint,
        alias: None,
        verify: Default::default(),
    };

    Ok(Node::new(kp, db, backend, registry, cfg))
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
