//! Discovery integration test: spin up two real HTTP listeners on
//! ephemeral ports, bootstrap node-b from node-a, then assert node-b's
//! local peer directory contains node-a.

use std::sync::Arc;

use n3ur0n_adapters::{Backend, echo::EchoBackend};
use n3ur0n_core::Keypair;
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_server::http;
use n3ur0n_storage::{open_in_memory, peers as peers_repo};
use tokio::net::TcpListener;

async fn build_node() -> Node {
    let kp = Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let decls = backend.describe().await.unwrap();
    let registry = CapabilityRegistry::from_decls(decls);
    Node::new(
        kp,
        db,
        backend,
        registry,
        NodeConfig {
            endpoint: None,
            ..Default::default()
        },
    )
}

async fn spawn_node(node: Node) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let app = http::app(node, None);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{local}")
}

#[tokio::test]
async fn bootstrap_populates_directory() {
    let node_a = build_node().await;
    let node_b = build_node().await;

    let id_a = node_a.instance_id();
    let id_b = node_b.instance_id();

    let endpoint_a = spawn_node(node_a.clone()).await;

    // Sanity: node_b directory is empty before bootstrap.
    assert!(peers_repo::list(node_b.db(), 100).unwrap().is_empty());

    let outcomes =
        n3ur0n_node::discovery::bootstrap_initial_peers(&node_b, std::slice::from_ref(&endpoint_a))
            .await;
    assert_eq!(outcomes.len(), 1);
    let outcome = &outcomes[0];
    assert_eq!(outcome.error, None, "bootstrap reported error: {outcome:?}");
    assert_eq!(outcome.instance_id.as_deref(), Some(id_a.as_str()));

    let directory = peers_repo::list(node_b.db(), 100).unwrap();
    assert_eq!(directory.len(), 1);
    let entry = &directory[0];
    assert_eq!(entry.id, id_a.as_str());
    assert_eq!(entry.endpoint, endpoint_a);
    assert!(entry.describe_self_cached.is_some());

    // Idempotency: a second bootstrap should not duplicate the row.
    let _ = n3ur0n_node::discovery::bootstrap_initial_peers(&node_b, &[endpoint_a]).await;
    let directory = peers_repo::list(node_b.db(), 100).unwrap();
    assert_eq!(directory.len(), 1);

    // node_a's directory remains empty (b never advertised itself to a).
    assert!(peers_repo::list(node_a.db(), 100).unwrap().is_empty());

    let _ = id_b;
}

#[tokio::test]
async fn cascade_finds_third_party() {
    // Topology: a knows b, b knows c. After cascade discover from a for the
    // `echo` capability, a should have c in its directory too.
    let node_a = build_node().await;
    let node_b = build_node().await;
    let node_c = build_node().await;

    let endpoint_a = spawn_node(node_a.clone()).await;
    let endpoint_b = spawn_node(node_b.clone()).await;
    let endpoint_c = spawn_node(node_c.clone()).await;

    let _ = endpoint_a;
    // Seed: a knows b
    n3ur0n_node::discovery::bootstrap_initial_peers(&node_a, std::slice::from_ref(&endpoint_b))
        .await;
    // Seed: b knows c (so b can advertise c on get_known_peers)
    n3ur0n_node::discovery::bootstrap_initial_peers(&node_b, std::slice::from_ref(&endpoint_c))
        .await;

    // a now cascades: should reach b and discover c.
    let added = n3ur0n_node::discovery::discover_capability(&node_a, "echo")
        .await
        .unwrap();
    assert_eq!(added, 1, "expected one new peer (c)");

    let dir = peers_repo::list(node_a.db(), 100).unwrap();
    let ids: Vec<&str> = dir.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&node_b.instance_id().as_str()));
    assert!(ids.contains(&node_c.instance_id().as_str()));
}
