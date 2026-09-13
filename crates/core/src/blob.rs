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
    /// Human-readable name the producer wants this blob to carry.
    ///
    /// Absent on most refs: the bytes are the identity, the name is a label.
    /// A capability that transforms a file sets it so the caller stores the
    /// result under a meaningful name instead of the provisional one derived
    /// from the capability and a timestamp. Sanitized by the receiver — it is
    /// a label chosen by a remote peer, never a path to trust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    if hex.len() != 64
        || !hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
    {
        return Err(CoreError::InvalidIdentifier(
            "blob hash must be 64 lowercase hex digits".into(),
        ));
    }
    Ok(())
}

/// Compute the canonical content hash for a byte sequence.
pub fn hash_bytes(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!(
        "{BLOB_HASH_PREFIX}{}",
        data_encoding::HEXLOWER.encode(&digest)
    )
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

/// Maximum length of a sanitized blob path, in bytes.
pub const MAX_BLOB_PATH_LEN: usize = 255;

/// Normalize a user-supplied blob path into something safe to store and display.
///
/// A blob path is a *local petname*: it never travels on the wire and is never
/// accepted from a remote peer. It is purely a display/lookup convenience over
/// the content hash, which stays the canonical identifier.
///
/// The rules exist because the raw input comes from a browser file picker (and
/// later from a rename box), so it must not be trusted as a filesystem path:
///
/// - backslashes are folded to `/` so Windows names keep their structure;
/// - `.` and `..` segments are dropped, which removes path traversal;
/// - leading/trailing and repeated separators collapse;
/// - control characters are stripped (they corrupt terminals and UI labels);
/// - the result is truncated to [`MAX_BLOB_PATH_LEN`] bytes on a char boundary.
///
/// Returns `None` when nothing usable remains, so callers can store SQL NULL
/// rather than an empty string.
#[must_use]
pub fn sanitize_blob_path(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .filter(|c| !c.is_control())
        .collect();

    let mut segments: Vec<&str> = Vec::new();
    for seg in cleaned.split('/') {
        let seg = seg.trim();
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        segments.push(seg);
    }
    if segments.is_empty() {
        return None;
    }

    let mut out = segments.join("/");
    if out.len() > MAX_BLOB_PATH_LEN {
        let mut cut = MAX_BLOB_PATH_LEN;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        let trimmed = out.trim_end_matches('/').trim();
        if trimmed.is_empty() {
            return None;
        }
        out = trimmed.to_string();
    }
    Some(out)
}

/// File extension to use for a blob of the given MIME type.
///
/// The table covers what capabilities actually return; anything else falls
/// back to the subtype (`application/x-foo` → `x-foo`), and to `bin` when even
/// that is unusable. This is cosmetic — the MIME stays authoritative.
fn extension_for_mime(mime: &str) -> String {
    let mime = mime.split(';').next().unwrap_or("").trim().to_lowercase();
    let known = match mime.as_str() {
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        "text/csv" => Some("csv"),
        "text/html" => Some("html"),
        "application/json" => Some("json"),
        "application/pdf" => Some("pdf"),
        "application/zip" => Some("zip"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "audio/mpeg" => Some("mp3"),
        "audio/wav" => Some("wav"),
        "video/mp4" => Some("mp4"),
        _ => None,
    };
    if let Some(ext) = known {
        return ext.to_string();
    }
    let subtype = mime.rsplit('/').next().unwrap_or("");
    let subtype = subtype.split('+').next().unwrap_or("");
    let cleaned: String = subtype
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if cleaned.is_empty() {
        "bin".to_string()
    } else {
        cleaned
    }
}

/// Provisional path for a blob produced by a capability.
///
/// Outputs arrive nameless: the producer returns `{hash, size, mime}` and a
/// name is never accepted from the wire. The consumer therefore assigns one,
/// shaped `<capability>/<YYYY-MM-DD-HHMMSS>.<ext>`, so results group by the
/// capability that made them and sort chronologically.
///
/// It is deliberately provisional — the user renames it when it matters. Two
/// outputs of the same capability within the same second collide, which is
/// fine: a path is not a key, the hash is.
#[must_use]
pub fn derive_output_path(capability: &str, mime: &str, unix_ts: i64) -> String {
    let cap: String = capability
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    let cap = cap.trim_matches(['.', '-', '_']).to_string();
    let cap = if cap.is_empty() {
        "output".to_string()
    } else {
        cap
    };

    let stamp = time::OffsetDateTime::from_unix_timestamp(unix_ts)
        .ok()
        .map_or_else(
            || "unknown".to_string(),
            |t| {
                format!(
                    "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
                    t.year(),
                    u8::from(t.month()),
                    t.day(),
                    t.hour(),
                    t.minute(),
                    t.second()
                )
            },
        );

    format!("{cap}/{stamp}.{}", extension_for_mime(mime))
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

    #[test]
    fn sanitize_path_keeps_plain_name() {
        assert_eq!(
            sanitize_blob_path("rapport.pdf").as_deref(),
            Some("rapport.pdf")
        );
    }

    #[test]
    fn sanitize_path_keeps_folders() {
        assert_eq!(
            sanitize_blob_path("contrats/2026/bail.pdf").as_deref(),
            Some("contrats/2026/bail.pdf")
        );
    }

    #[test]
    fn sanitize_path_strips_traversal() {
        assert_eq!(
            sanitize_blob_path("../../keys.json").as_deref(),
            Some("keys.json")
        );
        assert_eq!(
            sanitize_blob_path("/etc/passwd").as_deref(),
            Some("etc/passwd")
        );
        assert_eq!(
            sanitize_blob_path("a/./b/../c.txt").as_deref(),
            Some("a/b/c.txt")
        );
    }

    #[test]
    fn sanitize_path_folds_backslashes() {
        assert_eq!(
            sanitize_blob_path("C:\\Users\\me\\note.txt").as_deref(),
            Some("C:/Users/me/note.txt")
        );
    }

    #[test]
    fn sanitize_path_strips_control_chars() {
        assert_eq!(
            sanitize_blob_path("rap\u{0}po\u{7}rt.pdf").as_deref(),
            Some("rapport.pdf")
        );
    }

    #[test]
    fn sanitize_path_rejects_empty() {
        assert!(sanitize_blob_path("").is_none());
        assert!(sanitize_blob_path("   ").is_none());
        assert!(sanitize_blob_path("../..").is_none());
        assert!(sanitize_blob_path("///").is_none());
    }

    #[test]
    fn sanitize_path_truncates_on_char_boundary() {
        let long = format!("{}.pdf", "é".repeat(400));
        let out = sanitize_blob_path(&long).unwrap();
        assert!(out.len() <= MAX_BLOB_PATH_LEN);
        assert!(
            out.chars()
                .all(|c| c == 'é' || c == '.' || c == 'p' || c == 'd' || c == 'f')
        );
    }

    #[test]
    fn derives_output_path_from_cap_and_mime() {
        // 2026-09-12T15:30:12Z
        let p = derive_output_path("translate", "text/plain", 1_789_227_012);
        assert_eq!(p, "translate/2026-09-12-153012.txt");
    }

    #[test]
    fn derives_output_path_stamp_is_utc() {
        assert_eq!(
            derive_output_path("sum", "application/pdf", 0),
            "sum/1970-01-01-000000.pdf"
        );
    }

    #[test]
    fn derives_output_path_sanitizes_capability() {
        assert_eq!(
            derive_output_path("../weird cap", "application/json", 0),
            "weird-cap/1970-01-01-000000.json"
        );
        assert_eq!(
            derive_output_path("", "text/csv", 0),
            "output/1970-01-01-000000.csv"
        );
    }

    #[test]
    fn extension_falls_back_to_subtype() {
        assert_eq!(extension_for_mime("application/x-tar"), "xtar");
        assert_eq!(extension_for_mime("text/plain; charset=utf-8"), "txt");
        assert_eq!(extension_for_mime("application/vnd.oasis+xml"), "vndoasis");
        assert_eq!(extension_for_mime(""), "bin");
    }

    #[test]
    fn derived_output_path_survives_sanitize() {
        let p = derive_output_path("translate", "text/plain", 0);
        assert_eq!(sanitize_blob_path(&p).as_deref(), Some(p.as_str()));
    }
}
