//! Mock-server tests for `OpenAIBackend`.
//!
//! Spins up a tiny axum service that mimics the chat-completions endpoint
//! and exercises the adapter end-to-end without touching the network.

use axum::{Json, Router, extract::State, routing::post};
use n3ur0n_adapters::{
    Backend,
    openai::{OpenAIBackend, OpenAIConfig},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockState {
    last_request: Arc<Mutex<Option<Value>>>,
}

async fn chat(State(state): State<MockState>, Json(req): Json<Value>) -> Json<Value> {
    *state.last_request.lock().await = Some(req.clone());
    let echoed = req
        .get("messages")
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Json(json!({
        "id": "chatcmpl-1",
        "model": req.get("model"),
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": format!("echo: {echoed}")}
        }]
    }))
}

async fn spawn_mock() -> (String, MockState) {
    let state = MockState::default();
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

#[tokio::test]
async fn invoke_with_prompt_string() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "test-model".into(),
        api_key: None,
        description: None,
    })
    .unwrap();

    let out = backend
        .invoke("chat", json!({"prompt": "hello"}))
        .await
        .unwrap();

    assert_eq!(out["message"]["role"], "assistant");
    assert_eq!(out["message"]["content"], "echo: hello");
    assert_eq!(out["finish_reason"], "stop");

    let req = state.last_request.lock().await.clone().unwrap();
    assert_eq!(req["model"], "test-model");
    assert_eq!(req["stream"], false);
    assert_eq!(req["messages"][0]["role"], "user");
    assert_eq!(req["messages"][0]["content"], "hello");
}

#[tokio::test]
async fn invoke_with_messages_array_passes_through() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
    })
    .unwrap();

    let payload = json!({
        "model": "explicit-model",
        "messages": [
            {"role": "system", "content": "you are short"},
            {"role": "user", "content": "ping"}
        ],
        "temperature": 0.1
    });
    let _ = backend.invoke("chat", payload).await.unwrap();

    let req = state.last_request.lock().await.clone().unwrap();
    assert_eq!(req["model"], "explicit-model");
    assert_eq!(req["temperature"], 0.1);
    assert_eq!(req["messages"][0]["role"], "system");
    assert_eq!(req["stream"], false);
}

#[tokio::test]
async fn unknown_capability_rejected() {
    let backend = OpenAIBackend::new(OpenAIConfig::ollama_local("nope")).unwrap();
    let err = backend.invoke("not-chat", json!({})).await.unwrap_err();
    matches!(err, n3ur0n_adapters::AdapterError::UnknownCapability(_));
}

#[tokio::test]
async fn describe_lists_chat_capability() {
    let backend = OpenAIBackend::new(OpenAIConfig::ollama_local("qwen2.5:0.5b")).unwrap();
    let decls = backend.describe().await.unwrap();
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "chat");
    assert!(
        decls[0].description.contains("qwen2.5:0.5b"),
        "default-model name should appear in description: {}",
        decls[0].description
    );
}
