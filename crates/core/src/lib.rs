//! `n3ur0n-core`: pure protocol logic.
//!
//! This crate **must not** depend on HTTP, databases, or any IO facade. It is
//! intentionally narrow: data types, cryptographic identity, canonical
//! signing/verification, and the typed payload schemas of the four protocol
//! verbs (`describe_self`, `get_known_peers`, `ping`, `invoke`).
//!
//! Reference: `n3ur0n-architecture-v0.md` §5–§9.

#![deny(missing_docs)]

/// Capability declaration as exposed by `describe_self`.
pub mod capability;
/// Strongly-typed core errors.
pub mod error;
/// Cryptographic identity: keypair, public key, instance id.
pub mod identity;
/// Wire envelope, signed message, JCS-canonical signing helpers.
pub mod message;
/// Typed payloads for the four v0.1 protocol verbs.
pub mod protocol;
/// Pure verification of a `SignedMessage` (signature, recipient, clock skew).
pub mod verify;

pub use error::{CoreError, CoreResult};
pub use identity::{InstanceId, Keypair, PublicKey};
pub use message::{Envelope, ProtocolVerb, SignedMessage};
pub use verify::{Clock, SystemClock, VerifiedEnvelope, VerifyConfig, verify_envelope};
