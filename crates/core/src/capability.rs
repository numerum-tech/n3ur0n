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
///
/// v0.1.1 adds planner-oriented metadata (`examples`, `disambiguation`,
/// `negative_examples`, `output_semantic`).
///
/// v0.2 (protocol "n3ur0n/0.2") adds publisher versioning + localisation:
/// `version` (semver, mandatory for new publishers; defaults to "0.0.0"
/// for backward compat when receiving from legacy peers), plus optional
/// `languages` (BCP 47) and `countries` (ISO 3166-1 alpha-2) lists.
///
/// All new fields default to empty/None so older publishers deserialize
/// without breaking; the planner side enforces `examples.len() >= 1` for
/// its own catalog inclusion in v0.2.
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

    // ---- v0.1.1 planner-oriented metadata --------------------------------
    /// Canonical usage examples. The planner injects these into the
    /// compile prompt so a small LLM (7-13B) can pattern-match intent →
    /// (capability, args) reliably. Empty list = legacy publisher; v0.2
    /// planners log a warning and skip the cap from their catalog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<CapabilityExample>,
    /// Free-form text disambiguating this capability from similarly-named
    /// or overlapping ones ("prefer this when …, do not confuse with …").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disambiguation: Option<String>,
    /// Intents that look like a match but should NOT trigger this cap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub negative_examples: Vec<NegativeExample>,
    /// Short prose describing what the output *means* (not its JSON
    /// structure). Helps the reflection step compose the user-facing
    /// reply without hallucinating semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_semantic: Option<String>,

    // ---- v0.2 publisher metadata -----------------------------------------
    /// Semver of the capability content itself (independent of
    /// `PROTOCOL_VERSION`). Lets consumers detect when a publisher updates
    /// a cap. Default `"0.0.0"` for legacy peers that omit the field.
    #[serde(default = "default_cap_version")]
    pub version: String,
    /// BCP 47 language tags the cap operates in (e.g. `["fr", "en"]`).
    /// Empty list = language-agnostic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// ISO 3166-1 alpha-2 country codes the cap is meaningful or
    /// available in (e.g. `["FR", "BE"]`). Empty list = unrestricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub countries: Vec<String>,
}

fn default_cap_version() -> String {
    "0.0.0".to_string()
}

/// One canonical example of how to call a capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityExample {
    /// Natural-language user intent this example covers.
    pub user_intent: String,
    /// Args the planner should emit for this intent.
    pub args: Value,
    /// Expected output shape (or a representative value).
    pub expected_output: Value,
}

/// A user intent that should *not* invoke this capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NegativeExample {
    /// The misleading user intent.
    pub user_intent: String,
    /// Why this cap is the wrong choice (often points to the right one).
    pub why_not: String,
}
