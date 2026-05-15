//! Integration test: drive the axum router with `tower::ServiceExt::oneshot`,
//! covering the happy path (signed `ping`) and a tampered envelope.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use n3ur0n_adapters::{Backend, echo::EchoBackend};
use n3ur0n_core::message::{Envelope, ProtocolVerb};
use n3ur0n_core::{Keypair, SignedMessage};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_server::http::app;
use n3ur0n_storage::open_in_memory;
use serde_json::json;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

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
            endpoint: Some("https://srv.example".into()),
            alias: None,
            ..Default::default()
        },
    )
}

async fn post_message(router: &axum::Router, msg: &SignedMessage) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/n3ur0n/v0/messages")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(msg).unwrap()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

#[tokio::test]
async fn signed_ping_round_trip() {
    let node = build_node().await;
    let recipient = node.instance_id();
    let router = app(node, None);

    let client = Keypair::generate();
    let env = Envelope {
        sender_id: client.instance_id(),
        recipient_id: recipient.clone(),
        timestamp: OffsetDateTime::now_utc(),
        nonce: Uuid::new_v4().to_string(),
        verb: ProtocolVerb::Ping,
        payload: json!({}),
        sender_endpoint: None,
    };
    let signed = env.sign(&client).unwrap();

    let (status, body) = post_message(&router, &signed).await;
    assert_eq!(status, StatusCode::OK);

    let reply: SignedMessage = serde_json::from_value(body).unwrap();
    reply.verify_signature().unwrap();
    assert_eq!(reply.envelope.recipient_id, client.instance_id());
    assert_eq!(reply.envelope.sender_id, recipient);
}

#[tokio::test]
async fn tampered_signature_returns_unauthorized() {
    let node = build_node().await;
    let recipient = node.instance_id();
    let router = app(node, None);

    let client = Keypair::generate();
    let mut signed = Envelope {
        sender_id: client.instance_id(),
        recipient_id: recipient,
        timestamp: OffsetDateTime::now_utc(),
        nonce: Uuid::new_v4().to_string(),
        verb: ProtocolVerb::Ping,
        payload: json!({}),
        sender_endpoint: None,
    }
    .sign(&client)
    .unwrap();

    // Replace first hex char with a different valid hex digit so the
    // signature stays decodable but mathematically invalid.
    let mut chars: Vec<char> = signed.signature.chars().collect();
    chars[0] = if chars[0] == '0' { '1' } else { '0' };
    signed.signature = chars.into_iter().collect();

    let (status, body) = post_message(&router, &signed).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "signature");
}

#[tokio::test]
async fn replay_returns_conflict() {
    let node = build_node().await;
    let recipient = node.instance_id();
    let router = app(node, None);

    let client = Keypair::generate();
    let signed = Envelope {
        sender_id: client.instance_id(),
        recipient_id: recipient,
        timestamp: OffsetDateTime::now_utc(),
        nonce: Uuid::new_v4().to_string(),
        verb: ProtocolVerb::Ping,
        payload: json!({}),
        sender_endpoint: None,
    }
    .sign(&client)
    .unwrap();

    let (s1, _) = post_message(&router, &signed).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = post_message(&router, &signed).await;
    assert_eq!(s2, StatusCode::CONFLICT);
    assert_eq!(body["error"], "replay");
}

#[tokio::test]
async fn api_locales_lists_embedded_catalogs() {
    let node = build_node().await;
    let router = app(node, None);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v0/locales")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let codes: Vec<String> = body["available"]
        .as_array()
        .expect("available is an array")
        .iter()
        .filter_map(|e| e.get("code").and_then(|v| v.as_str()).map(String::from))
        .collect();
    // At minimum the en + fr catalogs ship with the binary today.
    assert!(codes.contains(&"en".to_string()), "missing en in {:?}", codes);
    assert!(codes.contains(&"fr".to_string()), "missing fr in {:?}", codes);
    // Default is documented as "en" — guard against accidental flip.
    assert_eq!(body["default"], "en");
}
