//! Capability descriptors for the integration tests, read from the manifests
//! the cluster actually serves (`docker/manifests/`).
//!
//! These used to come from `UtilityBackend::describe()`, a compiled-in backend
//! that carried 300 lines of descriptor metadata duplicating what the manifests
//! now hold. Two copies of the same descriptor drift; reading the real files
//! means a test grades the planner on what a node really publishes.

use std::path::PathBuf;

use n3ur0n_core::capability::CapabilityDecl;

/// Every cap manifest committed for the test cluster, in a stable order.
///
/// Panics rather than returning an error: a test that cannot find the
/// manifests is misconfigured, not failing.
pub(crate) fn cluster_cap_decls() -> Vec<CapabilityDecl> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docker/manifests");
    let mut decls: Vec<CapabilityDecl> = ["node-a", "node-b"]
        .iter()
        .flat_map(|node| {
            let dir = root.join(node).join("caps");
            n3ur0n_node::manifest::load_cap_dir(&dir)
                .into_iter()
                .map(move |r| {
                    r.unwrap_or_else(|e| panic!("cap manifest in {}: {e}", dir.display()))
                        .descriptor
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        !decls.is_empty(),
        "no cap manifests under {}",
        root.display()
    );
    decls.sort_by(|a, b| a.name.cmp(&b.name));
    decls
}
