//! Errors produced by the node orchestration layer.

use thiserror::Error;

/// Result alias for node operations.
pub type NodeResult<T> = Result<T, NodeError>;

/// Errors produced while orchestrating a request through the node.
///
/// These errors are intentionally distinct from
/// [`n3ur0n_core::CoreError`]: a `NodeError` may *contain* a `CoreError` but
/// also covers IO, anti-replay storage, and adapter failures that the core
/// layer knows nothing about.
#[derive(Debug, Error)]
pub enum NodeError {
    /// Verification (signature/recipient/clock) failed.
    #[error(transparent)]
    Core(#[from] n3ur0n_core::CoreError),

    /// Storage subsystem failed.
    #[error(transparent)]
    Storage(#[from] n3ur0n_storage::StorageError),

    /// Backend adapter returned an error.
    #[error(transparent)]
    Adapter(#[from] n3ur0n_adapters::AdapterError),

    /// Anti-replay rejected the nonce.
    #[error("nonce already seen (replay)")]
    Replay,

    /// Capability not registered locally.
    #[error("capability not found: {0}")]
    UnknownCapability(String),

    /// Protocol violation: payload didn't match the verb.
    #[error("invalid payload for verb: {0}")]
    InvalidPayload(String),

    /// Backends reloaded but the cap rebind that follows did not. Carries
    /// the count so a caller can report what really happened instead of
    /// collapsing a partial success into a zero.
    #[error("backends reloaded ({backends_loaded}) but cap rebind failed: {reason}")]
    PartialReload {
        backends_loaded: usize,
        reason: String,
    },

    /// Identity file IO error.
    #[error("identity file: {0}")]
    Identity(String),

    /// Generic IO error (config dirs, key files).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialisation error from a node-level boundary.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// The user addressed something this node does not know — an unknown
    /// peer, lobe or capability — so the request was refused rather than
    /// answered against a wider scope than the one asked for.
    ///
    /// Carrying the tokens lets a caller point at what is wrong instead of
    /// restating it in prose: naming the mistake is the interface's job, not
    /// a model's.
    #[error("unknown reference: {}", .0.join(", "))]
    UnresolvedMentions(Vec<String>),

    /// Template substitution failed (missing path, unknown root, etc).
    #[error("template: {0}")]
    Template(#[from] crate::bindings::template::TemplateError),
}
