//! Capability declaration as exposed in `describe_self`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Free,
    Restricted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDecl {
    pub name: String,
    pub description: String,
    pub schema_in: Value,
    pub schema_out: Value,
    pub mode: AccessMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub lobe_ids: Vec<String>,
}
