//! A file staged locally and then sent to a peer must stop calling itself
//! staged.
//!
//! The blob index keys on the hash and `upsert` never rewrites a row's
//! classification — deliberately, so a later write cannot relabel an inbound
//! result or a cap staging blob. The cost was that the only blobs eligible for
//! an outbound `PUT` are exactly the ones already indexed as class D, so the
//! class A promotion the spec describes never happened: the Files panel's
//! Outbound section could not fill, and a file that had travelled still
//! displayed as "local cache, staged".

use std::sync::Arc;

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_core::Keypair;
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig, blob_resolve};
use n3ur0n_storage::open_in_memory;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A peer that answers health, has never seen the blob, and accepts the PUT.
/// The blob routes match on method alone: the hash travels percent-encoded
/// (`sha256%3A…`), so a path matcher written with the raw hash silently misses
/// and every request 404s.
async fn peer_accepting_uploads(peer: &MockServer, instance_id: &str, hash: &str, size: usize) {
    Mock::given(method("GET"))
        .and(path("/n3ur0n/v0/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok",
            "instance_id": instance_id,
            "protocol_version": "n3ur0n/0.3",
        })))
        .mount(peer)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(404))
        .mount(peer)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "hash": hash,
            "size": size,
        })))
        .expect(1)
        .mount(peer)
        .await;
}

async fn node_with_blobs(dir: &std::path::Path) -> Node {
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let decls = backend.describe().await.unwrap();
    Node::new(
        Keypair::generate(),
        open_in_memory().unwrap(),
        backend,
        CapabilityRegistry::from_decls(decls),
        NodeConfig {
            blobs_dir: Some(dir.to_path_buf()),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn a_staged_blob_becomes_outbound_once_it_is_uploaded_to_a_peer() {
    let dir = tempfile::tempdir().unwrap();
    let node = node_with_blobs(dir.path()).await;

    let staged =
        blob_resolve::store_local_cache(&node, b"hello n3ur0n", "text/plain", Some("x.txt"), None, None)
            .unwrap();

    let before = n3ur0n_storage::blobs::get(node.db(), &staged.hash)
        .unwrap()
        .unwrap();
    assert_eq!(before.anchor_kind, "local_cache", "staged file starts class D");
    assert_eq!(before.processing_status, "staged");

    let peer = MockServer::start().await;
    let peer_kp = Keypair::generate();
    peer_accepting_uploads(
        &peer,
        peer_kp.instance_id().as_str(),
        &staged.hash,
        staged.size as usize,
    )
    .await;

    let http = reqwest::Client::new();
    let args = json!({
        "document": { "hash": staged.hash, "size": staged.size, "mime": staged.mime }
    });
    blob_resolve::prepare_invoke_args(&node, &http, &peer.uri(), "translate", args)
        .await
        .unwrap();

    let after = n3ur0n_storage::blobs::get(node.db(), &staged.hash)
        .unwrap()
        .unwrap();
    assert_eq!(
        after.anchor_kind, "user_session",
        "a blob that has been PUT at a peer is class A, not local cache"
    );
    assert_eq!(after.processing_status, "referenced");
    assert_eq!(after.provenance, "outbound");
    assert_eq!(after.role, "input");
    // The human-readable name survives the promotion.
    assert_eq!(after.path.as_deref(), Some("x.txt"));
}

#[tokio::test]
async fn an_inbound_result_is_never_relabelled_as_outbound() {
    let dir = tempfile::tempdir().unwrap();
    let node = node_with_blobs(dir.path()).await;

    // Same bytes, but indexed as a class B result of a remote invoke.
    let bytes = b"downloaded";
    let hash = n3ur0n_core::hash_bytes(bytes);
    std::fs::write(dir.path().join(&hash), bytes).unwrap();
    n3ur0n_storage::blobs::upsert(
        node.db(),
        &n3ur0n_storage::blobs::BlobInsert {
            hash: hash.clone(),
            path: Some("result.txt".into()),
            size: bytes.len() as i64,
            mime: "text/plain".into(),
            expires_at: 4_102_444_800,
            storage_path: dir.path().join(&hash).display().to_string(),
            provenance: "inbound".into(),
            role: "output".into(),
            anchor_kind: "user_session".into(),
            processing_status: "ready".into(),
            local_user_id: None,
            client_id: Some("client-a".into()),
            conversation_id: None,
            dispatch_id: None,
            capability: Some("translate".into()),
            remote_sender_id: None,
            ticket_nonce: None,
            invoke_id: None,
            user_visible: true,
            user_deletable: true,
            uploader_id: None,
            recipients_whitelist: None,
        },
    )
    .unwrap();

    let peer = MockServer::start().await;
    let peer_kp = Keypair::generate();
    peer_accepting_uploads(&peer, peer_kp.instance_id().as_str(), &hash, bytes.len()).await;

    let http = reqwest::Client::new();
    let args = json!({ "document": { "hash": hash, "size": bytes.len(), "mime": "text/plain" } });
    blob_resolve::prepare_invoke_args(&node, &http, &peer.uri(), "translate", args)
        .await
        .unwrap();

    let after = n3ur0n_storage::blobs::get(node.db(), &hash).unwrap().unwrap();
    assert_eq!(after.provenance, "inbound", "a result stays a result");
    assert_eq!(after.role, "output");
}
