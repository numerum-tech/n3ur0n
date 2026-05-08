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
async fn invoke_with_messages_array_locks_model_to_default() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
    })
    .unwrap();

    let payload = json!({
        // Client tries to override — we ignore and pin to default.
        "model": "explicit-model",
        "messages": [
            {"role": "system", "content": "you are short"},
            {"role": "user", "content": "ping"}
        ],
        "temperature": 0.1
    });
    let _ = backend.invoke("chat", payload).await.unwrap();

    let req = state.last_request.lock().await.clone().unwrap();
    // The operator-configured default wins.
    assert_eq!(req["model"], "default");
    assert_eq!(req["temperature"], 0.1);
    assert_eq!(req["messages"][0]["role"], "system");
    assert_eq!(req["stream"], false);
}

#[tokio::test]
async fn drops_caller_supplied_tools_and_tool_call_history() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
    })
    .unwrap();

    // Caller — typically a planner LLM forwarding args verbatim — leaks
    // `tools` and `tool_calls` into the chat cap. Both must be dropped.
    let payload = json!({
        "messages": [
            {"role": "user", "content": "hi"},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "c1", "function": {"name": "x", "arguments": "{}"}}]
            },
            {"role": "tool", "tool_call_id": "c1", "content": "{}"}
        ],
        "tools": [{"type": "function", "function": {"name": "ghost"}}],
        "tool_choice": "auto",
        "model": "evil"
    });
    let _ = backend.invoke("chat", payload).await.unwrap();

    let req = state.last_request.lock().await.clone().unwrap();
    assert_eq!(req["model"], "default");
    assert!(req.get("tools").is_none(), "tools must be stripped");
    assert!(req.get("tool_choice").is_none(), "tool_choice must be stripped");
    let msgs = req["messages"].as_array().unwrap();
    for m in msgs {
        assert!(m.get("tool_calls").is_none(), "tool_calls in history stripped");
        assert!(m.get("tool_call_id").is_none(), "tool_call_id stripped");
        // `role: tool` demoted to system.
        assert_ne!(m["role"].as_str().unwrap(), "tool");
    }
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
