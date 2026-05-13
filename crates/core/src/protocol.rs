//! Typed payloads for the four v0.1 protocol verbs.
//!
//! Each verb has a request and a response payload. They are serialised as the
//! `payload` field of an [`Envelope`](crate::message::Envelope). The wire shape
//! is intentionally explicit (no untyped `serde_json::Value` reaching this
//! layer) so that protocol changes show up as compile errors.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::CapabilityDecl;
use crate::identity::InstanceId;

/// Protocol version string returned by `describe_self`.
///
/// v0.1.1: adds optional planner metadata fields to `CapabilityDecl`
/// (`examples`, `disambiguation`, `negative_examples`, `output_semantic`).
///
/// v0.2: adds `CapabilityDecl.version` (semver, defaults to "0.0.0" on
/// legacy input), plus optional `languages` (BCP 47) and `countries`
/// (ISO 3166-1 alpha-2) lists. Backwards-compatible at the serde level —
/// older publishers still validate because every new field has a default.
pub const PROTOCOL_VERSION: &str = "n3ur0n/0.3";

// ---------------------------------------------------------------------------
// describe_self
// ---------------------------------------------------------------------------

/// Empty request payload for `describe_self`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DescribeSelfRequest {}

/// Response payload for `describe_self`. Mirrors architecture spec §8.3.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DescribeSelfResponse {
    /// Canonical instance id (must equal the envelope's `sender_id`).
    pub instance_id: InstanceId,
    /// Public endpoint where this instance is reachable, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Optional human alias (e.g. `@alice`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Implementation-defined protocol version string.
    pub protocol_version: String,
    /// RFC 3339 timestamp of when this descriptor was last updated.
    pub updated_at: String,
    /// Capabilities exposed by this instance.
    #[serde(default)]
    pub capabilities: Vec<CapabilityDecl>,
}

// ---------------------------------------------------------------------------
// get_known_peers
// ---------------------------------------------------------------------------

/// Request payload for `get_known_peers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetKnownPeersRequest {
    /// Maximum number of peers to return. Servers may return fewer.
    pub limit: u32,
    /// Optional capability-name filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

/// One peer descriptor in `get_known_peers` responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerSummary {
    /// Canonical id.
    pub instance_id: InstanceId,
    /// Reachable endpoint, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Alias, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

/// Response payload for `get_known_peers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetKnownPeersResponse {
    /// Peer entries from the local directory. Order is not significant.
    pub peers: Vec<PeerSummary>,
}

// ---------------------------------------------------------------------------
// ping
// ---------------------------------------------------------------------------

/// Empty request payload for `ping`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PingRequest {}

/// Response payload for `ping`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PingResponse {
    /// RFC 3339 server-side timestamp.
    pub server_time: String,
}

// ---------------------------------------------------------------------------
// invoke
// ---------------------------------------------------------------------------

/// Request payload for `invoke`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeRequest {
    /// Capability name as advertised by `describe_self`.
    pub capability: String,
    /// Capability-specific arguments. Schema declared by the capability.
    pub args: Value,
    /// Opaque token attached when calling a capability in `restricted` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_token: Option<String>,
}

/// Response payload for `invoke`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeResponse {
    /// Capability output. Schema declared by the capability.
    pub result: Value,
}
