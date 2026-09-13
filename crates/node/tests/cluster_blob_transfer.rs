//! Real blob transfer over the wire, against the docker cluster.
//!
//! Ignored by default — it needs the cluster running:
//!
//! ```sh
//! docker compose -f docker/compose.yml up -d --build node-a node-b
//! cargo test -p n3ur0n-node --test cluster_blob_transfer -- --ignored --nocapture
//! ```
//!
//! What it exercises that a mock cannot: node-b's real listener, its ticket
//! verification, its on-disk store and its class C indexing. The sending side
//! runs the actual `prepare_invoke_args` path with its own identity — a peer
//! is a keypair, so an ad-hoc one is as legitimate to node-b as node-a's.

use std::sync::Arc;

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_core::Keypair;
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig, blob_client, blob_resolve, client};
use n3ur0n_storage::open_in_memory;
use serde_json::json;

/// node-b in `docker/compose.yml` — the utility backend, host port 4243.
const PEER: &str = "http://localhost:4243";

#[tokio::test]
#[ignore = "needs the docker cluster: docker compose -f docker/compose.yml up -d node-a node-b"]
async fn a_file_travels_to_a_live_peer_and_comes_back_identical() {
    let dir = tempfile::tempdir().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let decls = backend.describe().await.unwrap();
    let node = Node::new(
        Keypair::generate(),
        open_in_memory().unwrap(),
        backend,
        CapabilityRegistry::from_decls(decls),
        NodeConfig {
            blobs_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
    );

    // Unique bytes per run: node-b keeps what earlier runs sent, and a blob it
    // already holds short-circuits the upload on HEAD.
    let payload = format!(
        "n3ur0n cluster blob transfer {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bytes = payload.as_bytes();

    let staged =
        blob_resolve::store_local_cache(&node, bytes, "text/plain", Some("note.txt"), None, None)
            .unwrap();
    let before = n3ur0n_storage::blobs::get(node.db(), &staged.hash)
        .unwrap()
        .unwrap();
    assert_eq!(before.anchor_kind, "local_cache");

    let http = reqwest::Client::new();
    assert!(
        !blob_client::head_blob(&http, PEER, &staged.hash)
            .await
            .expect("node-b must answer HEAD — is the cluster up?"),
        "these bytes should be new to node-b"
    );

    let args = json!({ "text": { "hash": staged.hash, "size": staged.size, "mime": staged.mime } });
    blob_resolve::prepare_invoke_args(&node, &http, PEER, "reverse", args)
        .await
        .expect("upload to node-b failed");

    // Sender side: the blob left the local cache behind.
    let after = n3ur0n_storage::blobs::get(node.db(), &staged.hash)
        .unwrap()
        .unwrap();
    assert_eq!(after.anchor_kind, "user_session", "promoted to class A");
    assert_eq!(after.processing_status, "referenced");

    // Receiver side: node-b holds it, and hands back the very same bytes.
    assert!(
        blob_client::head_blob(&http, PEER, &staged.hash)
            .await
            .unwrap(),
        "node-b should now hold the blob"
    );
    let recipient = client::discover_recipient(&http, PEER).await.unwrap();
    let back = blob_client::download_blob(&http, node.keypair(), PEER, &recipient, &staged)
        .await
        .expect("node-b refused to serve the blob back");
    assert_eq!(back, bytes, "bytes round-tripped unchanged");

    // node-b indexed it as cap staging (class C), never as a user file.
    let staging: serde_json::Value = http
        .get(format!("{PEER}/api/v0/cap-jobs/blobs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mine = staging["blobs"]
        .as_array()
        .expect("cap-jobs listing")
        .iter()
        .find(|b| b["hash"] == staged.hash.as_str())
        .expect("node-b should list the blob as a cap job");
    assert_eq!(mine["anchor_kind"], "cap_job");
    assert_eq!(mine["provenance"], "inbound");
    assert_eq!(mine["role"], "input");

    let user_files: serde_json::Value = http
        .get(format!("{PEER}/api/v0/files"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !user_files["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["hash"] == staged.hash.as_str()),
        "class C must never surface in the user Files panel"
    );

    println!("hash      : {}", staged.hash);
    println!("sender    : {} -> {}", before.anchor_kind, after.anchor_kind);
    println!("node-b    : {}", mine["anchor_kind"]);
}
