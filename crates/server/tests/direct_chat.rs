//! Integration tests for conversation dispatch `mode: "direct"`.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::post};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use n3ur0n_adapters::{Backend, echo::EchoBackend, openai::OpenAIConfig};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_node::runtime::RuntimeConfig;
use n3ur0n_server::bootstrap::{self, PlannerKind};
use n3ur0n_server::http::app_for_test;
use n3ur0n_storage::open_in_memory;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct MockLlmState {
    last_model: Arc<Mutex<Option<String>>>,
}

async fn chat(State(state): State<MockLlmState>, Json(req): Json<Value>) -> Json<Value> {
    let model = req
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    *state.last_model.lock().await = model.clone();
    Json(json!({
        "id": "chatcmpl-1",
        "model": model,
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": "direct-ok"}
        }]
    }))
}

async fn spawn_mock_llm() -> (String, MockLlmState) {
    let state = MockLlmState::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

async fn build_test_app(mock_base: String, _mock_state: MockLlmState) -> axum::Router {
    let kp = n3ur0n_core::Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let registry = CapabilityRegistry::from_decls(backend.describe().await.unwrap());
    let node = Node::new(
        kp,
        db,
        backend,
        registry,
        NodeConfig::default(),
    );
    let rt = bootstrap::build_runtime(
        node.clone(),
        PlannerKind::PlanExec {
            backend: OpenAIConfig {
                base_url: mock_base,
                default_model: "planner-default".into(),
                api_key: None,
                description: None,
                allow_model_override: false,
            },
            model_hint: None,
        },
        RuntimeConfig::default(),
    )
    .unwrap();
    app_for_test(node, Some(Arc::new(rt)))
}

fn cookie_header(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap_or(set_cookie)
        .to_string()
}

async fn post_json(
    router: &axum::Router,
    method: Method,
    uri: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let req = req.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|h| h.to_str().ok())
        .map(|s| cookie_header(s));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}))
    };
    (status, body, set_cookie)
}

#[tokio::test]
async fn direct_mode_returns_reply_with_empty_trace() {
    let (mock_base, llm_state) = spawn_mock_llm().await;
    let router = build_test_app(mock_base, llm_state.clone()).await;

    let (_, create_body, cookie) =
        post_json(&router, Method::POST, "/api/v0/conversations", json!({}), None).await;
    assert_eq!(create_body["id"].as_str().is_some(), true);
    let cookie = cookie.expect("client_id cookie");
    let conv_id = create_body["id"].as_str().unwrap();

    let (status, body, _) = post_json(
        &router,
        Method::POST,
        &format!("/api/v0/conversations/{conv_id}/messages"),
        json!({"message": "hello", "mode": "direct"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reply"], "direct-ok");
    assert!(body["trace"].as_array().unwrap().is_empty());
    assert_eq!(
        *llm_state.last_model.lock().await,
        Some("planner-default".into())
    );
}

#[tokio::test]
async fn direct_mode_respects_model_override() {
    let (mock_base, llm_state) = spawn_mock_llm().await;
    let router = build_test_app(mock_base, llm_state.clone()).await;

    let (_, create_body, cookie) =
        post_json(&router, Method::POST, "/api/v0/conversations", json!({}), None).await;
    let cookie = cookie.unwrap();
    let conv_id = create_body["id"].as_str().unwrap();

    let (status, _, _) = post_json(
        &router,
        Method::POST,
        &format!("/api/v0/conversations/{conv_id}/messages"),
        json!({
            "message": "pick a model",
            "mode": "direct",
            "model": "my-custom-model"
        }),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        *llm_state.last_model.lock().await,
        Some("my-custom-model".into())
    );
}

#[tokio::test]
async fn invalid_mode_returns_400() {
    let (mock_base, llm_state) = spawn_mock_llm().await;
    let router = build_test_app(mock_base, llm_state).await;

    let (_, create_body, cookie) =
        post_json(&router, Method::POST, "/api/v0/conversations", json!({}), None).await;
    let cookie = cookie.unwrap();
    let conv_id = create_body["id"].as_str().unwrap();

    let (status, _, _) = post_json(
        &router,
        Method::POST,
        &format!("/api/v0/conversations/{conv_id}/messages"),
        json!({"message": "hi", "mode": "magic"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn no_planner_runtime_returns_503() {
    let kp = n3ur0n_core::Keypair::generate();
    let db = open_in_memory().unwrap();
    let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
    let registry = CapabilityRegistry::from_decls(backend.describe().await.unwrap());
    let node = Node::new(kp, db, backend, registry, NodeConfig::default());
    let router = app_for_test(node, None);

    let (_, create_body, cookie) =
        post_json(&router, Method::POST, "/api/v0/conversations", json!({}), None).await;
    let cookie = cookie.unwrap();
    let conv_id = create_body["id"].as_str().unwrap();

    let (status, _, _) = post_json(
        &router,
        Method::POST,
        &format!("/api/v0/conversations/{conv_id}/messages"),
        json!({"message": "hi", "mode": "direct"}),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
