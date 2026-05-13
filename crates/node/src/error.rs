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

    /// Identity file IO error.
    #[error("identity file: {0}")]
    Identity(String),

    /// Generic IO error (config dirs, key files).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialisation error from a node-level boundary.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// Template substitution failed (missing path, unknown root, etc).
    #[error("template: {0}")]
    Template(#[from] crate::bindings::template::TemplateError),
}
