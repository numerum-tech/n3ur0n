//! In-memory capability registry.
//!
//! v0.1: declarations live in process and are populated from the active
//! backend adapter at startup. Persistent declarations (in the
//! `capabilities` SQLite table) are sync'd from this in-memory view.

use std::collections::HashMap;

use n3ur0n_core::capability::CapabilityDecl;

/// Lookup-friendly registry of capabilities exposed by this instance.
#[derive(Debug, Default, Clone)]
pub struct CapabilityRegistry {
    by_name: HashMap<String, CapabilityDecl>,
}

impl CapabilityRegistry {
    /// Build a registry from a slice of declarations. Last declaration wins
    /// on duplicate names.
    pub fn from_decls(decls: impl IntoIterator<Item = CapabilityDecl>) -> Self {
        let mut by_name = HashMap::new();
        for d in decls {
            by_name.insert(d.name.clone(), d);
        }
        Self { by_name }
    }

    /// Look up a capability by name.
    pub fn get(&self, name: &str) -> Option<&CapabilityDecl> {
        self.by_name.get(name)
    }

    /// Snapshot of all declarations, in insertion order is **not** guaranteed.
    pub fn all(&self) -> Vec<CapabilityDecl> {
        self.by_name.values().cloned().collect()
    }

    /// Number of registered capabilities.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
