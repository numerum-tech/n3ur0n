//! In-memory capability registry.
//!
//! v0.1: declarations live in process and are populated from the active
//! backend adapter at startup. Persistent declarations (in the
//! `capabilities` SQLite table) are sync'd from this in-memory view.
//!
//! v0.3 extension: when caps are loaded from manifests, each entry also
//! carries an `Arc<dyn Binding>` that knows how to invoke the upstream.
//! The `handler.rs::invoke` path tries the binding first; if absent
//! (legacy compile-time mode) it falls back to the single backend
//! injected into [`Node`](crate::node::Node).

use std::collections::HashMap;
use std::sync::Arc;

use n3ur0n_core::capability::CapabilityDecl;

use crate::bindings::Binding;

#[derive(Clone)]
struct Entry {
    decl: CapabilityDecl,
    binding: Option<Arc<dyn Binding>>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("name", &self.decl.name)
            .field("has_binding", &self.binding.is_some())
            .finish()
    }
}

/// Lookup-friendly registry of capabilities exposed by this instance.
#[derive(Debug, Default, Clone)]
pub struct CapabilityRegistry {
    by_name: HashMap<String, Entry>,
}

impl CapabilityRegistry {
    /// Build a registry from a slice of declarations (no bindings — legacy
    /// compile-time backend mode). Last declaration wins on duplicate
    /// names.
    pub fn from_decls(decls: impl IntoIterator<Item = CapabilityDecl>) -> Self {
        let mut by_name = HashMap::new();
        for d in decls {
            by_name.insert(
                d.name.clone(),
                Entry {
                    decl: d,
                    binding: None,
                },
            );
        }
        Self { by_name }
    }

    /// Build a registry from manifest entries, each carrying its own
    /// binding. Used by the v0.3 manifest-mode bootstrap.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (CapabilityDecl, Arc<dyn Binding>)>,
    ) -> Self {
        let mut by_name = HashMap::new();
        for (decl, binding) in entries {
            by_name.insert(
                decl.name.clone(),
                Entry {
                    decl,
                    binding: Some(binding),
                },
            );
        }
        Self { by_name }
    }

    /// Look up a capability declaration by name.
    pub fn get(&self, name: &str) -> Option<&CapabilityDecl> {
        self.by_name.get(name).map(|e| &e.decl)
    }

    /// Look up the binding for a capability, if one was registered.
    ///
    /// Returns `None` in legacy compile-time mode where capability
    /// invocations dispatch via `Node::backend()` instead.
    pub fn binding_for(&self, name: &str) -> Option<Arc<dyn Binding>> {
        self.by_name.get(name).and_then(|e| e.binding.clone())
    }

    /// Snapshot of all declarations, insertion order is **not** guaranteed.
    pub fn all(&self) -> Vec<CapabilityDecl> {
        self.by_name.values().map(|e| e.decl.clone()).collect()
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
