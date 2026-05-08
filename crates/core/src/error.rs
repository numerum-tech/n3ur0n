use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid identifier format: {0}")]
    InvalidIdentifier(String),

    #[error("signature verification failed")]
    SignatureInvalid,

    #[error("recipient mismatch: expected {expected}, got {actual}")]
    RecipientMismatch { expected: String, actual: String },

    #[error("timestamp out of acceptable window")]
    TimestampOutOfWindow,

    #[error("nonce already seen (replay)")]
    ReplayDetected,

    #[error("canonical serialization failed: {0}")]
    Canonical(String),

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
