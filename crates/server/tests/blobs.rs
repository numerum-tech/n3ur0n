//! Integration tests for blob PUT/GET/HEAD.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use n3ur0n_adapters::Backend;
use n3ur0n_core::blob::{BLOB_TICKET_HEADER, hash_bytes};
use n3ur0n_node::blob_client::{forge_get_ticket, forge_put_ticket};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_server::http;
use n3ur0n_storage::open_in_memory;
use tempfile::TempDir;
use tower::ServiceExt;

async fn test_node(dir: &std::path::Path) -> Node {
    let kp = n3ur0n_core::Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend = std::sync::Arc::new(n3ur0n_adapters::echo::EchoBackend);
    let decls = backend.describe().await.unwrap();
    let registry = CapabilityRegistry::from_decls(decls);
    let config = NodeConfig {
        endpoint: Some("http://localhost:4242".into()),
        ..Default::default()
    };
    let _ = dir;
    Node::new(kp, db, backend, registry, config)
}

#[tokio::test]
async fn blob_put_get_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let node = test_node(tmp.path()).await;
    let kp = node.keypair().clone();
    let recipient = node.instance_id();
    let runtime = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(None));
    let app = http::app_with_settings(node, runtime, Some(tmp.path().to_path_buf()), None, None);

    let data = b"hello blob world";
    let hash = hash_bytes(data);

    let put_ticket = forge_put_ticket(
        &kp,
        &recipient,
        &hash,
        data.len() as u64,
        "text/plain",
        "echo", // default echo backend cap
    )
    .unwrap();
    let put_header = n3ur0n_core::encode_ticket_wire(&put_ticket).unwrap();

    let put_req = Request::builder()
        .method("PUT")
        .uri(format!("/n3ur0n/v0/blobs/{hash}"))
        .header(BLOB_TICKET_HEADER, put_header)
        .header("content-type", "application/octet-stream")
        .body(Body::from(data.to_vec()))
        .unwrap();

    let put_resp = app.clone().oneshot(put_req).await.unwrap();
    assert_eq!(put_resp.status(), StatusCode::CREATED);

    let head_req = Request::builder()
        .method("HEAD")
        .uri(format!("/n3ur0n/v0/blobs/{hash}"))
        .body(Body::empty())
        .unwrap();
    let head_resp = app.clone().oneshot(head_req).await.unwrap();
    assert_eq!(head_resp.status(), StatusCode::OK);

    let get_ticket = forge_get_ticket(&kp, &recipient, &hash).unwrap();
    let get_header = n3ur0n_core::encode_ticket_wire(&get_ticket).unwrap();
    let get_req = Request::builder()
        .method("GET")
        .uri(format!("/n3ur0n/v0/blobs/{hash}"))
        .header(BLOB_TICKET_HEADER, get_header)
        .body(Body::empty())
        .unwrap();
    let get_resp = app.oneshot(get_req).await.unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let bytes = get_resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), data);
}
