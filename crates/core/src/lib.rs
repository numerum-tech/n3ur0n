//! n3ur0n-core
//!
//! Protocol types, crypto identity, message signing and verification for the
//! N3UR0N peer-to-peer gateway protocol. Reference spec: `n3ur0n-architecture-v0.md`.
//!
//! No HTTP, no SQL: this crate must stay a pure logic / data layer.

pub mod identity;
pub mod message;
pub mod capability;
pub mod error;

pub use error::{CoreError, CoreResult};
pub use identity::{InstanceId, Keypair, PublicKey};
pub use message::{Envelope, ProtocolVerb, SignedMessage};
