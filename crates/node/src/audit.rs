//! Classification of an exchange's outcome for the call journal.
//!
//! The journal feeds a dashboard, so `status` is a small closed vocabulary a
//! chart can group by — never the error message, which is unbounded and would
//! turn every ranking into a list of one.

pub use n3ur0n_storage::audit::Direction;

use crate::error::NodeError;

/// Short, stable kind for an error. Grouping by this is what makes "23 calls,
/// 4 refused" meaningful.
pub fn status_of(e: &NodeError) -> String {
    match e {
        NodeError::UnknownCapability(_) => "unknown_capability",
        NodeError::Replay => "replay",
        NodeError::InvalidPayload(_) => "invalid_payload",
        NodeError::UnresolvedMentions(_) => "unresolved_mentions",
        NodeError::Adapter(_) => "adapter_error",
        NodeError::Template(_) => "template_error",
        NodeError::Storage(_) => "storage_error",
        // Verification failures — bad signature, wrong recipient, clock
        // skew — all arrive as Core. They are the interesting half of
        // "external access": a node under a bad actor sees them pile up.
        NodeError::Core(_) => "rejected",
        _ => "error",
    }
    .to_string()
}
