//! Name-addressed registry of live backend instances.
//!
//! Built once at bootstrap from `backends/*.toml` manifests. Capabilities
//! reference backends by name (via `BindingSpec::backend(&self)`); this
//! registry resolves the reference.

use std::collections::HashMap;

use crate::bindings::{build_backend_instance, BackendInstance};
use crate::error::{NodeError, NodeResult};
use crate::manifest::BackendManifest;

#[derive(Debug, Default, Clone)]
pub struct BackendsRegistry {
    by_name: HashMap<String, BackendInstance>,
}

impl BackendsRegistry {
    /// Materialise every manifest into a `BackendInstance` and store by
    /// name. Duplicate names overwrite — last write wins.
    pub fn from_manifests(
        manifests: impl IntoIterator<Item = BackendManifest>,
    ) -> NodeResult<Self> {
        let mut by_name = HashMap::new();
        for m in manifests {
            let name = m.name.clone();
            let instance = build_backend_instance(&m)?;
            by_name.insert(name, instance);
        }
        Ok(Self { by_name })
    }

    pub fn get(&self, name: &str) -> Option<&BackendInstance> {
        self.by_name.get(name)
    }

    /// Resolve a name or return a clear error pointing at the offending
    /// cap manifest. Bindings call this during construction.
    pub fn require(&self, name: &str) -> NodeResult<&BackendInstance> {
        self.get(name).ok_or_else(|| {
            NodeError::InvalidPayload(format!(
                "capability references backend `{name}` which is not declared in backends/"
            ))
        })
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}
