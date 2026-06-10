//! Blob protocol types: BlobRef, ticket payload, classification enums.
//!
//! Bytes never travel inside signed `invoke` envelopes; they are referenced
//! by content-addressed hash and transferred via `/n3ur0n/v0/blobs`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

/// Canonical prefix for SHA-256 blob identifiers.
pub const BLOB_HASH_PREFIX: &str = "sha256:";

/// HTTP header carrying a base64url-encoded signed blob ticket.
pub const BLOB_TICKET_HEADER: &str = "X-N3UR0N-Ticket";

/// Reference to a blob inside an `invoke` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// Content-addressed identifier (`sha256:` + 64 lowercase hex digits).
    pub hash: String,
    /// Size in bytes.
    pub size: u64,
    /// Declared MIME type.
    pub mime: String,
    /// Canonical fetch URL on the publisher (present on outbound refs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_url: Option<String>,
}

/// Blob ticket operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlobOperation {
    /// Upload bytes to the publisher.
    Put,
    /// Download bytes from the publisher.
    Get,
    /// Delete a blob the sender uploaded.
    Delete,
}

/// Semantic purpose of a blob ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlobPurpose {
    /// Blob will be passed as an invoke argument.
    Input,
    /// Blob produced by an invocation; consumer downloads it.
    Output,
    /// Blob owned by the sender (for delete).
    Owned,
}

/// Signed ticket payload authorizing a single blob HTTP operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobTicketPayload {
    /// HTTP operation this ticket authorizes.
    pub operation: BlobOperation,
    /// Hash of the blob (`put` / `get`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Declared size (`put`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Declared MIME (`put`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// Target capability (`put`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Unix timestamp after which the ticket is invalid.
    pub expires_at: i64,
    /// Semantic role of the blob for lifecycle / classification.
    pub purpose: BlobPurpose,
    /// Optional requested TTL in seconds (publisher may grant less).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_ttl_secs: Option<u64>,
    /// Peers allowed to `GET` an output blob (JSON array of `n3:…` ids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipients_whitelist: Option<Vec<String>>,
}

/// Direction of blob flow relative to our instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlobProvenance {
    /// We uploaded to a remote peer.
    Outbound,
    /// A remote peer produced or we received locally.
    Inbound,
}

/// Input vs output relative to an invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlobRole {
    /// Invoke argument.
    Input,
    /// Invoke result.
    Output,
}

/// What the blob is anchored to in local storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorKind {
    /// User-initiated session (classes A, B).
    UserSession,
    /// Remote peer staging for our cap (class C).
    CapJob,
    /// Local file picker / cache (class D).
    LocalCache,
}

/// Local processing state (non-wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessingStatus {
    /// HTTP transfer in progress.
    Uploading,
    /// Present locally, not yet in an invoke.
    Staged,
    /// BlobRef included in an in-flight invoke.
    Referenced,
    /// Invoke / planner step running on peer.
    Processing,
    /// Available to the user.
    Ready,
    /// Cap consumed the blob; awaiting GC.
    Consumed,
    /// TTL exceeded.
    Expired,
    /// Stale / superseded.
    Stale,
}

/// Derived insert policy for the local blob index (§2.4).
#[derive(Debug, Clone, Copy)]
pub struct BlobClassification {
    /// Outbound vs inbound relative to our instance.
    pub provenance: BlobProvenance,
    /// Input vs output relative to an invoke.
    pub role: BlobRole,
    /// User session, cap job, or local cache anchor.
    pub anchor_kind: AnchorKind,
    /// Whether the blob appears in the user Files panel.
    pub user_visible: bool,
    /// Whether the local user may delete this blob.
    pub user_deletable: bool,
    /// Local processing lifecycle state.
    pub processing_status: ProcessingStatus,
}

/// Validate `sha256:` + 64 lowercase hex digits.
pub fn validate_hash(s: &str) -> CoreResult<()> {
    if !s.starts_with(BLOB_HASH_PREFIX) {
        return Err(CoreError::InvalidIdentifier(format!(
            "blob hash must start with {BLOB_HASH_PREFIX}"
        )));
    }
    let hex = &s[BLOB_HASH_PREFIX.len()..];
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()) {
        return Err(CoreError::InvalidIdentifier(
            "blob hash must be 64 lowercase hex digits".into(),
        ));
    }
    Ok(())
}

/// Compute the canonical content hash for a byte sequence.
pub fn hash_bytes(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!("{BLOB_HASH_PREFIX}{}", data_encoding::HEXLOWER.encode(&digest))
}

/// Classification when **we** upload to a remote peer before our invoke (class A).
pub fn classify_outbound_upload() -> BlobClassification {
    BlobClassification {
        provenance: BlobProvenance::Outbound,
        role: BlobRole::Input,
        anchor_kind: AnchorKind::UserSession,
        user_visible: true,
        user_deletable: true,
        processing_status: ProcessingStatus::Ready,
    }
}

/// Classification when we download a result blob for a local user (class B).
pub fn classify_inbound_output() -> BlobClassification {
    BlobClassification {
        provenance: BlobProvenance::Inbound,
        role: BlobRole::Output,
        anchor_kind: AnchorKind::UserSession,
        user_visible: true,
        user_deletable: true,
        processing_status: ProcessingStatus::Ready,
    }
}

/// Classification when a remote peer PUTs on **our** listener for our cap (class C).
pub fn classify_cap_staging() -> BlobClassification {
    BlobClassification {
        provenance: BlobProvenance::Inbound,
        role: BlobRole::Input,
        anchor_kind: AnchorKind::CapJob,
        user_visible: false,
        user_deletable: false,
        processing_status: ProcessingStatus::Staged,
    }
}

/// Classification for local file-picker cache (class D).
pub fn classify_local_cache() -> BlobClassification {
    BlobClassification {
        provenance: BlobProvenance::Outbound,
        role: BlobRole::Input,
        anchor_kind: AnchorKind::LocalCache,
        user_visible: true,
        user_deletable: true,
        processing_status: ProcessingStatus::Staged,
    }
}

/// Default blob TTL in seconds by purpose (§5.1).
pub fn default_ttl_secs(purpose: BlobPurpose) -> u64 {
    match purpose {
        BlobPurpose::Input => 60 * 60,
        BlobPurpose::Output => 24 * 60 * 60,
        BlobPurpose::Owned => 60 * 60,
    }
}

/// Encode a signed ticket for the `X-N3UR0N-Ticket` header (base64url, no padding).
pub fn encode_ticket_wire(signed: &crate::message::SignedMessage) -> CoreResult<String> {
    let json = serde_json::to_vec(signed).map_err(|e| CoreError::Canonical(e.to_string()))?;
    Ok(data_encoding::BASE64URL_NOPAD.encode(&json))
}

/// Decode a ticket from the `X-N3UR0N-Ticket` header.
pub fn decode_ticket_wire(header: &str) -> CoreResult<crate::message::SignedMessage> {
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(header.trim().as_bytes())
        .map_err(|e| CoreError::Crypto(format!("ticket base64url: {e}")))?;
    serde_json::from_slice(&bytes).map_err(CoreError::Serde)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_bytes_deterministic() {
        let h = hash_bytes(b"hello");
        assert!(h.starts_with(BLOB_HASH_PREFIX));
        validate_hash(&h).unwrap();
    }

    #[test]
    fn rejects_bad_hash() {
        assert!(validate_hash("sha256:ZZ").is_err());
        assert!(validate_hash("md5:abc").is_err());
    }
}
