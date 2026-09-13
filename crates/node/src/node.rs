//! Runtime n3ur0n node.

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use n3ur0n_adapters::Backend;
use n3ur0n_core::{Keypair, SystemClock, VerifyConfig};
use n3ur0n_storage::Db;

use crate::backends_registry::BackendsRegistry;
use crate::bindings::build_binding;
use crate::error::{NodeError, NodeResult};
use crate::manifest::{load_backend_dir, load_cap_dir};
use crate::registry::CapabilityRegistry;

/// Static configuration of a [`Node`].
#[derive(Debug, Clone, Default)]
pub struct NodeConfig {
    /// Public endpoint advertised in `describe_self`. `None` for consumers.
    pub endpoint: Option<String>,
    /// Optional human alias.
    pub alias: Option<String>,
    /// Lobes this instance claims membership of, advertised in
    /// `describe_self`. Bounded by
    /// [`MAX_LOBES_PER_INSTANCE`](n3ur0n_core::MAX_LOBES_PER_INSTANCE); a
    /// capability may only claim a lobe present here.
    pub lobe_ids: Vec<String>,
    /// Verification policy for incoming messages.
    pub verify: VerifyConfig,
    /// Initial peers to bootstrap from at startup.
    pub bootstrap_peers: Vec<String>,
    /// Local blob cache directory (`<config>/blobs/sha256/`). Required for
    /// planner blob orchestration and the Files panel.
    pub blobs_dir: Option<PathBuf>,
}

/// Log every lobe claim stripped by
/// [`CapabilityRegistry::enforce_instance_lobes`]. Silence would leave a user
/// wondering why `@lobe:medical` matches nothing while the manifest says it
/// should: the fix is to declare the lobe on the instance, and the warning has
/// to say so.
fn report_dropped_lobes(dropped_per_cap: Vec<(String, Vec<String>)>) {
    for (cap, dropped) in dropped_per_cap {
        tracing::warn!(
            cap = %cap,
            lobes = %dropped.join(", "),
            "capability claims lobes this instance does not declare; ignoring them \
             (declare them in Settings → Identity, or with `serve --lobe <id>`)"
        );
    }
}

/// Live node state. Cloning a [`Node`] is cheap: the heavy state sits behind
/// `Arc`s.
#[derive(Clone)]
pub struct Node {
    pub(crate) keypair: Arc<Keypair>,
    pub(crate) db: Db,
    pub(crate) backend: Arc<dyn Backend>,
    /// `Arc<ArcSwap<_>>` so the cap registry can be hot-swapped
    /// (cap.toml CRUD / file-watcher) without a node restart. Readers
    /// load a snapshot via [`Node::registry()`] which returns an
    /// `Arc<CapabilityRegistry>`; writers call
    /// [`Node::reload_caps_from`] to swap atomically.
    pub(crate) registry: Arc<ArcSwap<CapabilityRegistry>>,
    /// Manifest-mode backends registry, only set when the node was
    /// loaded from a `<config>/{backends,caps}/` directory. Used to
    /// re-bind capability entries when caps reload. `None` in legacy
    /// compile-time-backend mode. `ArcSwap` so a backend manifest CRUD
    /// can rebuild + swap atomically (then trigger a cap rebind).
    pub(crate) backends: Option<Arc<ArcSwap<BackendsRegistry>>>,
    /// Manifest dir (parent of `caps/` and `backends/`). Set when
    /// running in manifest mode so the cap-reload entry point can scan
    /// the same directory.
    pub(crate) manifest_dir: Option<PathBuf>,
    /// Live lobe membership. Seeded from [`NodeConfig::lobe_ids`] and
    /// swappable at runtime (settings → Lobes) like the cap registry, because
    /// joining a lobe should not require a restart. `describe_self` and the
    /// `cap.lobe_ids ⊆ instance.lobe_ids` rule both read *this*, never the
    /// startup copy.
    pub(crate) lobes: Arc<ArcSwap<Vec<String>>>,
    /// Live human alias, seeded from [`NodeConfig::alias`] and swappable at
    /// runtime. `describe_self` reads *this*, never the startup copy, so
    /// renaming an instance does not require a restart.
    pub(crate) alias: Arc<ArcSwap<Option<String>>>,
    pub(crate) config: NodeConfig,
    pub(crate) clock: Arc<dyn n3ur0n_core::Clock>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("instance_id", &self.keypair.instance_id())
            .field("config", &self.config)
            .field("capabilities", &self.registry.load().len())
            .field("manifest_dir", &self.manifest_dir)
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
        let mut registry = registry;
        report_dropped_lobes(registry.enforce_instance_lobes(&config.lobe_ids));
        Self {
            keypair: Arc::new(keypair),
            db,
            backend,
            registry: Arc::new(ArcSwap::from_pointee(registry)),
            backends: None,
            manifest_dir: None,
            lobes: Arc::new(ArcSwap::from_pointee(config.lobe_ids.clone())),
            alias: Arc::new(ArcSwap::from_pointee(config.alias.clone())),
            config,
            clock: Arc::new(SystemClock),
        }
    }

    /// Attach a `BackendsRegistry` + manifest dir to enable
    /// [`Node::reload_caps_from`]. Required for cap hot-reload.
    pub fn with_manifest_runtime(
        mut self,
        backends: Arc<BackendsRegistry>,
        manifest_dir: PathBuf,
    ) -> Self {
        // Unwrap the incoming Arc into the cell so existing call sites keep
        // their `Arc<BackendsRegistry>` ergonomics.
        let inner = Arc::try_unwrap(backends).unwrap_or_else(|a| (*a).clone());
        self.backends = Some(Arc::new(ArcSwap::from_pointee(inner)));
        self.manifest_dir = Some(manifest_dir);
        self
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

    /// Snapshot the current capability registry. The returned `Arc` is
    /// detached from the cell — a concurrent reload will not invalidate
    /// it, but the reader sees a frozen view.
    pub fn registry(&self) -> Arc<CapabilityRegistry> {
        self.registry.load_full()
    }

    /// Human alias this instance currently advertises, if any.
    pub fn alias(&self) -> Option<String> {
        self.alias.load().as_ref().clone()
    }

    /// Rename the instance. Caller validates
    /// ([`n3ur0n_core::validate_alias`]); this method trusts it.
    pub fn set_alias(&self, alias: Option<String>) {
        self.alias.store(Arc::new(alias));
    }

    /// Lobes this instance currently claims membership of.
    pub fn lobes(&self) -> Vec<String> {
        self.lobes.load().as_ref().clone()
    }

    /// Replace the live lobe set, then re-apply `cap.lobe_ids ⊆
    /// instance.lobe_ids` to the registry in place.
    ///
    /// Leaving a lobe strips it from every capability that claimed it. Joining
    /// one cannot restore a lobe already stripped from an in-memory
    /// declaration — that comes back on the next cap reload, which manifest
    /// mode performs right after this call. Caller validates the ids
    /// ([`n3ur0n_core::validate_instance_lobes`]); this method trusts them.
    pub fn set_lobes(&self, lobes: Vec<String>) {
        self.lobes.store(Arc::new(lobes.clone()));
        let mut registry = (*self.registry.load_full()).clone();
        report_dropped_lobes(registry.enforce_instance_lobes(&lobes));
        self.registry.store(Arc::new(registry));
    }

    /// Re-scan the configured manifest dir's `caps/` subfolder and
    /// atomically swap the capability registry. Caller MUST have built
    /// the node with [`Node::with_manifest_runtime`]; otherwise this
    /// returns an error.
    ///
    /// Returns the number of caps successfully registered after reload.
    pub fn reload_caps_from_manifest_dir(&self) -> NodeResult<usize> {
        let Some(dir) = self.manifest_dir.as_deref() else {
            return Err(NodeError::InvalidPayload(
                "node was not built in manifest mode — cap hot-reload unavailable".into(),
            ));
        };
        let Some(backends_cell) = self.backends.as_ref() else {
            return Err(NodeError::InvalidPayload(
                "node has no backends registry — cap hot-reload unavailable".into(),
            ));
        };
        let backends = backends_cell.load_full();
        let caps_dir = dir.join("caps");
        let mut entries: Vec<(
            n3ur0n_core::CapabilityDecl,
            Arc<dyn crate::bindings::Binding>,
        )> = Vec::new();
        for result in load_cap_dir(&caps_dir) {
            let cap = match result {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping malformed cap manifest on reload");
                    continue;
                }
            };
            let Some(backend_instance) = backends.get(cap.binding.backend()) else {
                tracing::warn!(
                    cap = %cap.descriptor.name,
                    backend = %cap.binding.backend(),
                    "cap references unknown backend on reload; skipping"
                );
                continue;
            };
            let binding = match build_binding(&cap.binding, backend_instance) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        cap = %cap.descriptor.name,
                        error = %e,
                        "binding construction failed on reload; skipping cap"
                    );
                    continue;
                }
            };
            entries.push((cap.descriptor, binding));
        }
        let mut new_registry = CapabilityRegistry::from_entries(entries);
        report_dropped_lobes(new_registry.enforce_instance_lobes(&self.lobes()));
        let len = new_registry.len();
        self.registry.store(Arc::new(new_registry));
        tracing::info!(loaded = len, "cap registry hot-reloaded");
        Ok(len)
    }

    /// Re-scan `<manifest_dir>/backends/*.toml`, build a fresh
    /// [`BackendsRegistry`], swap it in atomically, then trigger a cap
    /// rebind so live capability bindings point at the new backend
    /// instances. Returns `(backends_loaded, caps_loaded)`.
    ///
    /// Errors if the node was not built with [`Node::with_manifest_runtime`].
    /// Malformed individual backend files are logged + skipped (consistent
    /// with bootstrap-time behavior). If zero backends parse, the swap
    /// still happens — call sites should surface a warning when len() == 0.
    pub fn reload_backends_from_manifest_dir(&self) -> NodeResult<(usize, usize)> {
        let Some(dir) = self.manifest_dir.as_deref() else {
            return Err(NodeError::InvalidPayload(
                "node was not built in manifest mode — backend hot-reload unavailable".into(),
            ));
        };
        let Some(cell) = self.backends.as_ref() else {
            return Err(NodeError::InvalidPayload(
                "node has no backends registry — backend hot-reload unavailable".into(),
            ));
        };
        let backends_dir = dir.join("backends");
        let mut manifests = Vec::new();
        for result in load_backend_dir(&backends_dir) {
            match result {
                Ok(m) => manifests.push(m),
                Err(e) => {
                    tracing::warn!(error = %e, "skipping malformed backend manifest on reload");
                }
            }
        }
        let new_registry = BackendsRegistry::from_manifests(manifests)?;
        let backends_len = new_registry.len();
        cell.store(Arc::new(new_registry));
        tracing::info!(loaded = backends_len, "backends registry hot-reloaded");
        // Rebind caps against the new backends so existing bindings don't
        // hold stale references.
        let caps_len = self.reload_caps_from_manifest_dir()?;
        Ok((backends_len, caps_len))
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

    /// Whether this node was started in manifest mode (`backends/` + `caps/`).
    pub fn is_manifest_mode(&self) -> bool {
        self.manifest_dir.is_some()
    }

    /// True when a named manifest backend is loaded and is `openai_compat`.
    pub fn has_openai_compat_backend(&self, name: &str) -> bool {
        let Some(cell) = self.backends.as_ref() else {
            return false;
        };
        matches!(
            cell.load_full().get(name),
            Some(crate::bindings::BackendInstance::OpenAI(_))
        )
    }
}
