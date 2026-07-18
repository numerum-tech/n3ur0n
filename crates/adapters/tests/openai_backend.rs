//! Mock-server tests for `OpenAIBackend`.
//!
//! Spins up a tiny axum service that mimics the chat-completions endpoint
//! and exercises the adapter end-to-end without touching the network.

use axum::{Json, Router, extract::State, routing::post};
use n3ur0n_adapters::{
    Backend,
    openai::{OpenAIBackend, OpenAIConfig, normalize_openai_base_url},
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
        allow_model_override: false,
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
        allow_model_override: false,
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
async fn passes_tool_fields_through_for_planner_iteration() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
        allow_model_override: false,
    })
    .unwrap();

    // The planner uses OpenAIBackend for its own LLM call. It needs:
    //   - top-level `tools` to advertise capabilities to the model
    //   - prior `tool_calls` / `role: tool` history inside `messages` so
    //     the model can resume plan→call→observe across iterations
    // Top-level `model` override is still rejected (operator-locked).
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
    assert_eq!(req["model"], "default", "model must be locked");
    assert!(req.get("tools").is_some(), "tools forwarded");
    assert!(req.get("tool_choice").is_some(), "tool_choice forwarded");
    let msgs = req["messages"].as_array().unwrap();
    // History is preserved verbatim.
    assert!(msgs.iter().any(|m| m.get("tool_calls").is_some()));
    assert!(msgs.iter().any(|m| m["role"] == "tool"));
}

#[tokio::test]
async fn messages_as_json_string_is_coerced_to_array() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
        allow_model_override: false,
    })
    .unwrap();

    // Some 8B models emit `messages` as a stringified JSON array.
    let payload = json!({
        "messages": "[{\"role\":\"user\",\"content\":\"hi\"}]"
    });
    let _ = backend.invoke("chat", payload).await.unwrap();
    let req = state.last_request.lock().await.clone().unwrap();
    assert!(
        req["messages"].is_array(),
        "messages must be coerced to array"
    );
    assert_eq!(req["messages"][0]["content"], "hi");
}

#[tokio::test]
async fn messages_as_plain_string_falls_back_to_prompt() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
        allow_model_override: false,
    })
    .unwrap();

    let payload = json!({
        "messages": "this is not JSON"
    });
    let _ = backend.invoke("chat", payload).await.unwrap();
    let req = state.last_request.lock().await.clone().unwrap();
    assert!(req["messages"].is_array());
    assert_eq!(req["messages"][0]["role"], "user");
    assert_eq!(req["messages"][0]["content"], "this is not JSON");
}

#[tokio::test]
async fn unknown_capability_rejected() {
    let backend = OpenAIBackend::new(OpenAIConfig::ollama_local("nope")).unwrap();
    let err = backend.invoke("not-chat", json!({})).await.unwrap_err();
    matches!(err, n3ur0n_adapters::AdapterError::UnknownCapability(_));
}

#[tokio::test]
async fn allow_model_override_passes_caller_model() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
        allow_model_override: true,
    })
    .unwrap();

    let _ = backend
        .invoke(
            "chat",
            json!({
                "model": "custom-llm",
                "messages": [{"role": "user", "content": "hi"}]
            }),
        )
        .await
        .unwrap();

    let req = state.last_request.lock().await.clone().unwrap();
    assert_eq!(req["model"], "custom-llm");
}

#[tokio::test]
async fn allow_model_override_falls_back_to_default_when_absent() {
    let (base, state) = spawn_mock().await;
    let backend = OpenAIBackend::new(OpenAIConfig {
        base_url: base,
        default_model: "default".into(),
        api_key: None,
        description: None,
        allow_model_override: true,
    })
    .unwrap();

    let _ = backend
        .invoke(
            "chat",
            json!({"messages": [{"role": "user", "content": "hi"}]}),
        )
        .await
        .unwrap();

    let req = state.last_request.lock().await.clone().unwrap();
    assert_eq!(req["model"], "default");
}

#[test]
fn normalize_strips_ollama_native_api_path() {
    assert_eq!(
        normalize_openai_base_url("http://192.168.4.101:11434/api/generate"),
        "http://192.168.4.101:11434"
    );
    assert_eq!(
        normalize_openai_base_url("http://localhost:11434/v1/"),
        "http://localhost:11434"
    );
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
