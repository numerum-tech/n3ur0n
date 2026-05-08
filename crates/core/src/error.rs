//! Core errors. Stable, exhaustive — every variant maps to a distinct rejection
//! reason in the protocol layer.

use thiserror::Error;

/// Result alias for core operations.
pub type CoreResult<T> = Result<T, CoreError>;

/// All errors a protocol-level operation can produce. Higher layers should map
/// these to wire responses without losing information.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Identifier failed to parse (bad prefix, bad encoding, etc.).
    #[error("invalid identifier format: {0}")]
    InvalidIdentifier(String),

    /// Cryptographic signature did not verify against the claimed public key.
    #[error("signature verification failed")]
    SignatureInvalid,

    /// `recipient_id` of the envelope does not match the local instance.
    #[error("recipient mismatch: expected {expected}, got {actual}")]
    RecipientMismatch {
        /// Local instance id.
        expected: String,
        /// Value received in the envelope.
        actual: String,
    },

    /// Timestamp falls outside the allowed clock-skew window.
    #[error("timestamp out of acceptable window")]
    TimestampOutOfWindow,

    /// Anti-replay rejected the nonce.
    #[error("nonce already seen (replay)")]
    ReplayDetected,

    /// JCS canonical serialization failed.
    #[error("canonical serialization failed: {0}")]
    Canonical(String),

    /// Generic crypto error (decoding, key shape, etc.).
    #[error("crypto error: {0}")]
    Crypto(String),

    /// JSON (de)serialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
