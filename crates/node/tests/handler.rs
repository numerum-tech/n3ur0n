//! End-to-end tests for the verb dispatcher.

use std::sync::{Arc, Mutex};

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_core::message::{Envelope, ProtocolVerb};
use n3ur0n_core::protocol::{DescribeSelfResponse, InvokeRequest, InvokeResponse, PingResponse};
use n3ur0n_core::{Clock, Keypair};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig, handle_request};
use n3ur0n_storage::open_in_memory;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

struct FixedClock(Mutex<OffsetDateTime>);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.0.lock().unwrap()
    }
}

async fn make_node() -> (Node, Keypair) {
    let server_kp = Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let decls = backend.describe().await.unwrap();
    let registry = CapabilityRegistry::from_decls(decls);
    let node = Node::new(
        Keypair::from_secret_bytes(&server_kp.secret_bytes()),
        db,
        backend.clone(),
        registry,
        NodeConfig {
            endpoint: Some("https://srv.example".into()),
            alias: Some("@srv".into()),
            ..Default::default()
        },
    )
    .with_clock(Arc::new(FixedClock(Mutex::new(
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
    ))));
    (node, server_kp)
}

fn signed(
    sender: &Keypair,
    recipient_id: &n3ur0n_core::InstanceId,
    verb: ProtocolVerb,
    payload: serde_json::Value,
    ts: OffsetDateTime,
) -> n3ur0n_core::SignedMessage {
    Envelope {
        sender_id: sender.instance_id(),
        recipient_id: recipient_id.clone(),
        timestamp: ts,
        nonce: Uuid::new_v4().to_string(),
        verb,
        payload,
        sender_endpoint: None,
    }
    .sign(sender)
    .unwrap()
}

#[tokio::test]
async fn ping_round_trip() {
    let (node, _server_kp) = make_node().await;
    let client = Keypair::generate();
    let now = node.clock().now();

    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::Ping,
        json!({}),
        now,
    );
    let reply = handle_request(&node, req).await.unwrap();

    reply.verify_signature().unwrap();
    assert_eq!(reply.envelope.sender_id, node.instance_id());
    assert_eq!(reply.envelope.recipient_id, client.instance_id());
    let body: PingResponse = serde_json::from_value(reply.envelope.payload).unwrap();
    assert!(!body.server_time.is_empty());
}

#[tokio::test]
async fn describe_self_lists_capabilities() {
    let (node, _server_kp) = make_node().await;
    let client = Keypair::generate();
    let now = node.clock().now();

    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::DescribeSelf,
        json!({}),
        now,
    );
    let reply = handle_request(&node, req).await.unwrap();
    let body: DescribeSelfResponse = serde_json::from_value(reply.envelope.payload).unwrap();

    assert_eq!(body.instance_id, node.instance_id());
    assert_eq!(body.endpoint.as_deref(), Some("https://srv.example"));
    assert!(body.capabilities.iter().any(|c| c.name == "echo"));
}

#[tokio::test]
async fn replay_rejected() {
    let (node, _server_kp) = make_node().await;
    let client = Keypair::generate();
    let now = node.clock().now();

    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::Ping,
        json!({}),
        now,
    );
    handle_request(&node, req.clone()).await.unwrap();
    let err = handle_request(&node, req).await.unwrap_err();
    assert!(matches!(err, n3ur0n_node::NodeError::Replay));
}

#[tokio::test]
async fn invoke_echoes_args() {
    let (node, _server_kp) = make_node().await;
    let client = Keypair::generate();
    let now = node.clock().now();

    let payload = serde_json::to_value(InvokeRequest {
        capability: "echo".into(),
        args: json!({"hello": "world"}),
        subscription_token: None,
    })
    .unwrap();

    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::Invoke,
        payload,
        now,
    );
    let reply = handle_request(&node, req).await.unwrap();
    let body: InvokeResponse = serde_json::from_value(reply.envelope.payload).unwrap();
    assert_eq!(body.result, json!({"hello": "world"}));
}

#[tokio::test]
async fn invoke_unknown_capability_errors() {
    let (node, _server_kp) = make_node().await;
    let client = Keypair::generate();
    let now = node.clock().now();

    let payload = serde_json::to_value(InvokeRequest {
        capability: "nope".into(),
        args: json!({}),
        subscription_token: None,
    })
    .unwrap();

    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::Invoke,
        payload,
        now,
    );
    let err = handle_request(&node, req).await.unwrap_err();
    assert!(matches!(err, n3ur0n_node::NodeError::UnknownCapability(_)));
}

/// Manifest-mode end-to-end: build a node whose registry was populated
/// from a `BindingSpec::Http` against an in-test wiremock server. The
/// handler must route `invoke` through the cap's binding (not via
/// `node.backend()` which is the inert EchoBackend placeholder).
#[tokio::test]
async fn invoke_dispatches_via_manifest_binding() {
    use n3ur0n_node::backends_registry::BackendsRegistry;
    use n3ur0n_node::bindings::build_binding;
    use n3ur0n_node::manifest::{
        BackendKind as MfBackendKind, BackendManifest, BindingSpec, HttpBaseConfig, HttpMethod,
    };
    use std::collections::HashMap;
    use wiremock::matchers::{method as wm, path as wp};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(wm("GET"))
        .and(wp("/echo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pong": true})))
        .mount(&server)
        .await;

    let backends = BackendsRegistry::from_manifests(vec![BackendManifest {
        name: "test_http".into(),
        kind: MfBackendKind::HttpBase(HttpBaseConfig {
            base_url: server.uri(),
            headers: HashMap::new(),
        }),
    }])
    .unwrap();

    let spec = BindingSpec::Http {
        backend: "test_http".into(),
        url_template: "/echo".into(),
        method: HttpMethod::Get,
        headers: HashMap::new(),
        body_template: None,
        response_path: None,
        timeout_ms: None,
    };
    let binding = build_binding(&spec, backends.get("test_http").unwrap()).unwrap();

    let decl = n3ur0n_core::capability::CapabilityDecl {
        name: "ping_remote".into(),
        description: "pings the mock".into(),
        schema_in: json!({"type": "object"}),
        schema_out: json!({"type": "object"}),
        mode: n3ur0n_core::capability::AccessMode::Free,
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
    };
    let registry = CapabilityRegistry::from_entries(vec![(decl, binding)]);

    let server_kp = Keypair::generate();
    let db = open_in_memory().unwrap();
    let inert: Arc<dyn Backend> = Arc::new(EchoBackend);
    let node = Node::new(
        Keypair::from_secret_bytes(&server_kp.secret_bytes()),
        db,
        inert,
        registry,
        NodeConfig {
            endpoint: Some("https://srv.example".into()),
            alias: Some("@srv".into()),
            ..Default::default()
        },
    )
    .with_clock(Arc::new(FixedClock(Mutex::new(
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
    ))));

    let client = Keypair::generate();
    let now = node.clock().now();
    let payload = serde_json::to_value(InvokeRequest {
        capability: "ping_remote".into(),
        args: json!({}),
        subscription_token: None,
    })
    .unwrap();
    let req = signed(
        &client,
        &node.instance_id(),
        ProtocolVerb::Invoke,
        payload,
        now,
    );
    let reply = handle_request(&node, req).await.unwrap();
    let body: InvokeResponse = serde_json::from_value(reply.envelope.payload).unwrap();
    // Came from the wiremock, not from EchoBackend.
    assert_eq!(body.result, json!({"pong": true}));
}
