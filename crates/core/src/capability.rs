//! Capability declaration as exposed in `describe_self`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Access mode for a single capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    /// Any correctly signed message is accepted.
    Free,
    /// Caller must be in the whitelist or present a valid `subscription_token`.
    Restricted,
}

/// Wire-level capability declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityDecl {
    /// Capability name, unique within the instance.
    pub name: String,
    /// Free-form natural language description.
    pub description: String,
    /// JSON Schema of the input payload.
    pub schema_in: Value,
    /// JSON Schema of the output payload.
    pub schema_out: Value,
    /// Access mode declared for this specific capability.
    pub mode: AccessMode,
    /// Optional pricing string (free-form in v0.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<String>,
    /// Discovery tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Lobe identifiers this capability is attached to.
    #[serde(default)]
    pub lobe_ids: Vec<String>,
}
