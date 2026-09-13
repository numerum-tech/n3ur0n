//! A capability output must land in the requesting user's Files panel.
//!
//! Regression test: `record_inbound_output` used to index downloaded output
//! blobs with no owner at all, while `list_user_visible` selects on
//! `local_user_id = ? OR client_id = ?`. Every capability result was therefore
//! stored, counted against the GC, and invisible to the user who asked for it
//! — the "Inbound" category of the panel could never be anything but empty.

use n3ur0n_adapters::Backend;
use n3ur0n_core::BlobRef;
use n3ur0n_core::blob::{BLOB_TICKET_HEADER, hash_bytes};
use n3ur0n_node::blob_client::forge_put_ticket;
use n3ur0n_node::blob_resolve::{BlobOwner, fetch_output_blobs};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_server::http;
use n3ur0n_storage::{blobs, open_in_memory};
use serde_json::json;
use tempfile::TempDir;

async fn node_with(endpoint: Option<String>, blobs_dir: Option<std::path::PathBuf>) -> Node {
    let kp = n3ur0n_core::Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend = std::sync::Arc::new(n3ur0n_adapters::echo::EchoBackend);
    let decls = backend.describe().await.unwrap();
    let registry = CapabilityRegistry::from_decls(decls);
    let config = NodeConfig {
        endpoint,
        blobs_dir,
        ..Default::default()
    };
    Node::new(kp, db, backend, registry, config)
}

#[tokio::test]
async fn capability_output_lands_in_the_requesting_users_files() {
    let peer_tmp = TempDir::new().unwrap();
    let consumer_tmp = TempDir::new().unwrap();

    // Bind first so the publisher can advertise the address it actually serves.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer_endpoint = format!("http://{addr}");

    let peer = node_with(Some(peer_endpoint.clone()), None).await;
    let peer_id = peer.instance_id();
    let runtime = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let app = http::app_with_settings(
        peer,
        runtime,
        Some(peer_tmp.path().to_path_buf()),
        None,
        None,
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let consumer = node_with(None, Some(consumer_tmp.path().to_path_buf())).await;
    let http_client = reqwest::Client::new();

    // Stage the bytes on the publisher, signed by the consumer, so the later
    // GET is authorized through the ordinary uploader rule.
    let data = b"ceci est le resultat";
    let hash = hash_bytes(data);
    let ticket = forge_put_ticket(
        consumer.keypair(),
        &peer_id,
        &hash,
        data.len() as u64,
        "text/plain",
        "echo",
    )
    .unwrap();
    let put = http_client
        .put(format!("{peer_endpoint}/n3ur0n/v0/blobs/{hash}"))
        .header(
            BLOB_TICKET_HEADER,
            n3ur0n_core::encode_ticket_wire(&ticket).unwrap(),
        )
        .header("content-type", "text/plain")
        .body(data.to_vec())
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success(), "PUT failed: {}", put.status());

    // What a step returns: a result carrying a blob reference.
    let result = json!({
        "translation": {
            "hash": hash,
            "size": data.len(),
            "mime": "text/plain",
            "fetch_url": format!("{peer_endpoint}/n3ur0n/v0/blobs/{hash}"),
        }
    });

    let owner = BlobOwner {
        client_id: Some("client-a".into()),
        conversation_id: Some("conv-1".into()),
    };
    fetch_output_blobs(
        &consumer,
        &http_client,
        &peer_endpoint,
        "translate",
        &owner,
        result,
    )
    .await
    .expect("output blob download failed");

    // The panel query the UI actually runs.
    let listed = blobs::list_user_visible(consumer.db(), None, Some("client-a"), 50).unwrap();
    assert_eq!(listed.len(), 1, "output blob missing from the Files panel");
    let rec = &listed[0];
    assert_eq!(rec.hash, hash);
    assert_eq!(rec.capability.as_deref(), Some("translate"));
    assert_eq!(rec.conversation_id.as_deref(), Some("conv-1"));

    let path = rec.path.as_deref().expect("output blob has no path");
    assert!(path.starts_with("translate/"), "unexpected path: {path}");
    assert!(path.ends_with(".txt"), "unexpected path: {path}");

    // A different client must not see it.
    let other = blobs::list_user_visible(consumer.db(), None, Some("client-b"), 50).unwrap();
    assert!(other.is_empty(), "output blob leaked to another client");

    // And the BlobRef stays resolvable locally.
    let br = BlobRef {
        hash: hash.clone(),
        size: data.len() as u64,
        mime: "text/plain".into(),
        fetch_url: None,
        name: None,
    };
    assert_eq!(
        n3ur0n_node::blob_resolve::read_local_bytes(&consumer, &br.hash).as_deref(),
        Some(&data[..])
    );
}
