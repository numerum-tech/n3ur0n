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

    /// Subset of declarations that are network-visible (mode != Private).
    /// `describe_self` returns exactly this set; Private caps remain local.
    pub fn public_decls(&self) -> Vec<CapabilityDecl> {
        self.by_name
            .values()
            .filter(|e| e.decl.mode.is_public())
            .map(|e| e.decl.clone())
            .collect()
    }

    /// Apply the instance-level membership rule `cap.lobe_ids ⊆
    /// instance.lobe_ids`: any lobe a capability claims that its own instance
    /// does not declare is dropped from the declaration, so the registry, the
    /// local catalog and `describe_self` all agree on one set.
    ///
    /// Returns one entry per capability that lost at least one lobe, as
    /// `(capability name, dropped lobe ids)`, for the caller to report. The
    /// capability itself is kept — a stray lobe is a labelling mistake, not a
    /// reason to take a working skill off the network.
    pub fn enforce_instance_lobes(
        &mut self,
        instance_lobes: &[String],
    ) -> Vec<(String, Vec<String>)> {
        let mut dropped_per_cap = Vec::new();
        for (name, entry) in self.by_name.iter_mut() {
            let dropped: Vec<String> =
                n3ur0n_core::unclaimable_lobes(&entry.decl.lobe_ids, instance_lobes)
                    .into_iter()
                    .map(str::to_string)
                    .collect();
            if dropped.is_empty() {
                continue;
            }
            entry.decl.lobe_ids.retain(|l| instance_lobes.contains(l));
            dropped_per_cap.push((name.clone(), dropped));
        }
        dropped_per_cap.sort();
        dropped_per_cap
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

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_core::capability::AccessMode;
    use serde_json::json;

    fn decl(name: &str, mode: AccessMode) -> CapabilityDecl {
        CapabilityDecl {
            name: name.into(),
            description: "d".into(),
            schema_in: json!({}),
            schema_out: json!({}),
            mode,
            pricing: None,
            tags: vec![],
            lobe_ids: vec![],
            examples: vec![],
            disambiguation: None,
            negative_examples: vec![],
            output_semantic: None,
            version: "0.0.0".into(),
            languages: vec![],
            countries: vec![],
        }
    }

    fn decl_in_lobes(name: &str, lobes: &[&str]) -> CapabilityDecl {
        let mut d = decl(name, AccessMode::Free);
        d.lobe_ids = lobes.iter().map(|l| (*l).to_string()).collect();
        d
    }

    #[test]
    fn enforce_instance_lobes_strips_unclaimable_and_reports_them() {
        let mut reg = CapabilityRegistry::from_decls(vec![
            decl_in_lobes("a", &["medical", "finance"]),
            decl_in_lobes("b", &["medical"]),
            decl_in_lobes("c", &[]),
        ]);
        let dropped = reg.enforce_instance_lobes(&["medical".to_string()]);
        assert_eq!(dropped, vec![("a".to_string(), vec!["finance".to_string()])]);
        assert_eq!(reg.get("a").unwrap().lobe_ids, vec!["medical".to_string()]);
        assert_eq!(reg.get("b").unwrap().lobe_ids, vec!["medical".to_string()]);
        assert!(reg.get("c").unwrap().lobe_ids.is_empty());
        // Every capability survives: only the claim is dropped.
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn an_instance_with_no_lobes_grants_none() {
        let mut reg = CapabilityRegistry::from_decls(vec![decl_in_lobes("a", &["medical"])]);
        let dropped = reg.enforce_instance_lobes(&[]);
        assert_eq!(dropped.len(), 1);
        assert!(reg.get("a").unwrap().lobe_ids.is_empty());
    }

    #[test]
    fn public_decls_excludes_private() {
        let reg = CapabilityRegistry::from_decls(vec![
            decl("a", AccessMode::Free),
            decl("b", AccessMode::Restricted),
            decl("c", AccessMode::Private),
        ]);
        let mut names: Vec<String> = reg.public_decls().into_iter().map(|d| d.name).collect();
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        // `all()` still returns everything for local API consumers.
        assert_eq!(reg.all().len(), 3);
    }
}
