//! `n3ur0n-core`: pure protocol logic.
//!
//! This crate **must not** depend on HTTP, databases, or any IO facade. It is
//! intentionally narrow: data types, cryptographic identity, canonical
//! signing/verification, and the typed payload schemas of the four protocol
//! verbs (`describe_self`, `get_known_peers`, `ping`, `invoke`).
//!
//! Reference: `n3ur0n-architecture-v0.md` §5–§9.

#![deny(missing_docs)]

/// Blob protocol types (BlobRef, tickets, classification).
pub mod blob;
/// Capability declaration as exposed by `describe_self`.
pub mod capability;
/// Strongly-typed core errors.
pub mod error;
/// Cryptographic identity: keypair, public key, instance id.
pub mod identity;
/// Lobe identifiers and the instance/capability membership rules.
pub mod lobe;
/// Wire envelope, signed message, JCS-canonical signing helpers.
pub mod message;
/// Typed payloads for the four v0.1 protocol verbs.
pub mod protocol;
/// Pure verification of a `SignedMessage` (signature, recipient, clock skew).
pub mod verify;

pub use blob::{
    AnchorKind, BLOB_HASH_PREFIX, BLOB_TICKET_HEADER, BlobClassification, BlobOperation,
    BlobProvenance, BlobPurpose, BlobRef, BlobRole, BlobTicketPayload, MAX_BLOB_PATH_LEN,
    ProcessingStatus, classify_cap_staging, classify_inbound_output, classify_local_cache,
    classify_outbound_upload, decode_ticket_wire, default_ttl_secs, derive_output_path,
    encode_ticket_wire, hash_bytes, sanitize_blob_path, validate_hash,
};
pub use capability::{AccessMode, CapabilityDecl, CapabilityExample, NegativeExample};
pub use error::{CoreError, CoreResult};
pub use identity::{InstanceId, Keypair, PublicKey};
pub use lobe::{
    LOBE_ID_MAX_LEN, LOBE_ID_MIN_LEN, MAX_LOBES_PER_INSTANCE, unclaimable_lobes,
    validate_instance_lobes, validate_lobe_id,
};
pub use message::{Envelope, ProtocolVerb, SignedMessage};
pub use verify::{Clock, SystemClock, VerifiedEnvelope, VerifyConfig, verify_envelope};
