//! Runtime n3ur0n node.

use std::sync::Arc;

use n3ur0n_adapters::Backend;
use n3ur0n_core::{Keypair, SystemClock, VerifyConfig};
use n3ur0n_storage::Db;

use crate::registry::CapabilityRegistry;

/// Static configuration of a [`Node`].
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Public endpoint advertised in `describe_self`. `None` for consumers.
    pub endpoint: Option<String>,
    /// Optional human alias.
    pub alias: Option<String>,
    /// Verification policy for incoming messages.
    pub verify: VerifyConfig,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            alias: None,
            verify: VerifyConfig::default(),
        }
    }
}

/// Live node state. Cloning a [`Node`] is cheap: the heavy state sits behind
/// `Arc`s.
#[derive(Clone)]
pub struct Node {
    pub(crate) keypair: Arc<Keypair>,
    pub(crate) db: Db,
    pub(crate) backend: Arc<dyn Backend>,
    pub(crate) registry: Arc<CapabilityRegistry>,
    pub(crate) config: NodeConfig,
    pub(crate) clock: Arc<dyn n3ur0n_core::Clock>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("instance_id", &self.keypair.instance_id())
            .field("config", &self.config)
            .field("capabilities", &self.registry.len())
            .finish()
    }
}

impl Node {
    /// Build a new node from its parts. Caller is responsible for ensuring
    /// `registry` reflects what `backend` actually exposes.
    pub fn new(
        keypair: Keypair,
        db: Db,
        backend: Arc<dyn Backend>,
        registry: CapabilityRegistry,
        config: NodeConfig,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            db,
            backend,
            registry: Arc::new(registry),
            config,
            clock: Arc::new(SystemClock),
        }
    }

    /// Override the clock — only useful for tests.
    pub fn with_clock(mut self, clock: Arc<dyn n3ur0n_core::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Canonical instance id of this node.
    pub fn instance_id(&self) -> n3ur0n_core::InstanceId {
        self.keypair.instance_id()
    }

    /// Borrow the keypair (signing requires it).
    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    /// Access the immutable static config.
    pub fn config(&self) -> &NodeConfig {
        &self.config
    }

    /// Borrow the in-memory capability registry.
    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// Borrow the storage handle.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Borrow the backend adapter.
    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    /// Borrow the clock.
    pub fn clock(&self) -> &Arc<dyn n3ur0n_core::Clock> {
        &self.clock
    }
}
