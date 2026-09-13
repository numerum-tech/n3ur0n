//! The settings API must edit the directory the node actually loads.
//!
//! It used `config_dir/caps` unconditionally while the node reloads from
//! `--manifest-dir`. On a node started with the two pointing at different
//! places — every node of the Docker cluster — saving a capability wrote a
//! file nothing would ever read, reloaded the *other* directory, and answered
//! `{"ok": true, "registered": <count of the other dir>}`. A success message
//! for a no-op.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_node::backends_registry::BackendsRegistry;
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_server::http::app_with_settings_for_test;
use n3ur0n_storage::open_in_memory;
use serde_json::{Value, json};
use tower::ServiceExt;

fn hermes_backend_toml() -> &'static str {
    r#"
[manifest]
version = "0.1"

[backend]
name = "probe_api"
kind = "http_base"

[http_base]
base_url = "http://127.0.0.1:9"
"#
}

fn cap_request() -> Value {
    json!({
        "name": "probe",
        "version": "0.1.0",
        "description": "A capability saved through the settings API.",
        "schema_in": {"type": "object"},
        "schema_out": {"type": "object"},
        "examples": [{"user_intent": "probe", "args": {}, "expected_output": {}}],
        "binding": {
            "type": "http",
            "backend": "probe_api",
            "url_template": "/probe",
            "method": "POST"
        }
    })
}

#[tokio::test]
async fn a_saved_cap_lands_in_the_manifest_dir_not_the_config_dir() {
    let config_dir = tempfile::tempdir().unwrap();
    let manifest_dir = tempfile::tempdir().unwrap();
    // The manifest dir must already hold the backend the cap binds to,
    // otherwise the reload rejects the cap for an unknown backend.
    let backends_dir = manifest_dir.path().join("backends");
    std::fs::create_dir_all(&backends_dir).unwrap();
    std::fs::write(backends_dir.join("probe_api.toml"), hermes_backend_toml()).unwrap();
    std::fs::create_dir_all(manifest_dir.path().join("caps")).unwrap();

    let backends = BackendsRegistry::from_manifests(
        n3ur0n_node::manifest::load_backend_dir(&backends_dir)
            .into_iter()
            .map(|r| r.expect("backend manifest parses"))
            .collect::<Vec<_>>(),
    )
    .expect("build backends registry");

    let node = Node::new(
        n3ur0n_core::Keypair::generate(),
        open_in_memory().unwrap(),
        Arc::new(EchoBackend),
        CapabilityRegistry::default(),
        NodeConfig::default(),
    )
    .with_manifest_runtime(Arc::new(backends), manifest_dir.path().to_path_buf());

    let app = app_with_settings_for_test(node, None, config_dir.path().to_path_buf());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v0/caps/manifests")
                .header("content-type", "application/json")
                .body(Body::from(cap_request().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, StatusCode::OK, "save failed: {body}");

    assert!(
        manifest_dir.path().join("caps/probe.toml").exists(),
        "the cap must be written where the node loads from; got path {:?}",
        body.get("path")
    );
    assert!(
        !config_dir.path().join("caps/probe.toml").exists(),
        "nothing may be written to the config dir the node never reads"
    );
    // And it is live, not merely on disk: the reload counted it.
    assert_eq!(
        body.get("registered"),
        Some(&json!(1)),
        "the saved cap must be registered: {body}"
    );

    // The listing the UI renders reads the same directory.
    let listed = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v0/caps/manifests")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed: Value =
        serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        listed["dir"],
        json!(manifest_dir.path().join("caps").display().to_string())
    );
    assert_eq!(listed["caps"].as_array().map(Vec::len), Some(1), "{listed}");
}
