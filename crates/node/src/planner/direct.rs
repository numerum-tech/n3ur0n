//! Direct chat planner: single LLM call per message, no plan compilation.
//!
//! Unlike `PlanExecPlanner` which compiles a typed plan and executes it,
//! `DirectChatPlanner` skips planning and invokes the LLM directly with
//! the full conversation history. One LLM call per dispatch.

use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::Backend;
use serde_json::json;

use crate::conversation::{persist_last, ConversationState};
use crate::error::NodeResult;
use crate::node::Node;
use crate::planner::{
    DispatchEvent, DispatchMode, DispatchOptions, DispatchOutcome, EventSender, Planner,
    MAX_CONTEXT_TURNS,
};

/// Direct chat planner: calls LLM once per message without plan compilation.
///
/// Unlike `PlanExecPlanner` which compiles a typed plan and executes it across
/// the network, `DirectChatPlanner` invokes the LLM directly with the full
/// conversation history. One LLM call per user message, no tool calling,
/// no orchestration. Useful for casual chat when end-to-end latency matters.
///
/// Response format is strictly OpenAI-compatible: `{ model, message: { role, content }, finish_reason }`.
#[derive(Clone)]
pub struct DirectChatPlanner {
    /// Backend used for the single LLM call.
    pub llm_backend: Arc<dyn Backend>,
    /// Default model hint if no override is provided in opts.
    pub model_hint: Option<String>,
}

impl std::fmt::Debug for DirectChatPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectChatPlanner")
            .field("model_hint", &self.model_hint)
            .finish()
    }
}

impl DirectChatPlanner {
    /// Create a new direct planner with the given backend and optional model hint.
    ///
    /// # Arguments
    /// * `llm_backend` - the backend to invoke for each LLM call
    /// * `model_hint` - default model to use if no override is provided in dispatch options
    pub fn new(llm_backend: Arc<dyn Backend>, model_hint: Option<String>) -> Self {
        Self {
            llm_backend,
            model_hint,
        }
    }

    /// System prompt for direct chat: simple helpful assistant persona.
    ///
    /// Instructs the model to answer directly without claiming tool execution.
    /// Matches the spirit of `PlanExecPlanner`'s reflect-only mode.
    fn system_prompt() -> String {
        String::from(
            "You are a helpful assistant. Answer the user's question directly and honestly. \
            Do not claim to have executed any actions or tools — you can only provide advice."
        )
    }
}

#[async_trait]
impl Planner for DirectChatPlanner {
    async fn dispatch(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
        _mode: DispatchMode,
        opts: DispatchOptions,
    ) -> NodeResult<DispatchOutcome> {
        // Push user turn and persist
        state.push_user(user_message);
        persist_last(node.db(), state)
            .map_err(|e| crate::error::NodeError::InvalidPayload(format!("persist user: {e}")))?;

        // Build messages: system prompt + conversation history
        let mut messages = vec![json!({
            "role": "system",
            "content": Self::system_prompt()
        })];
        messages.extend(state.to_chat_messages(MAX_CONTEXT_TURNS));

        // Invoke LLM
        let model = opts.model_override.or_else(|| self.model_hint.clone());
        let payload = json!({
            "messages": messages,
            "temperature": 0.7,
            "model": model,
        });

        let response = self.llm_backend.invoke("chat", payload).await?;

        // Extract model and content from response
        let response_model = response
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let content = response
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::NodeError::InvalidPayload(
                    "LLM response missing message.content field".to_string(),
                )
            })?
            .to_string();

        // Push assistant turn and persist
        state.push_assistant(content.clone(), response_model.clone());
        persist_last(node.db(), state)
            .map_err(|e| crate::error::NodeError::InvalidPayload(format!("persist assistant: {e}")))?;

        // Return outcome with empty trace
        Ok(DispatchOutcome {
            reply: content,
            model: response_model,
            trace: vec![],
        })
    }

    async fn dispatch_streaming(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
        mode: DispatchMode,
        opts: DispatchOptions,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        // Emit PlanReady with empty steps
        let _ = events.send(DispatchEvent::PlanReady { steps: vec![] });

        // Emit Reflecting
        let _ = events.send(DispatchEvent::Reflecting);

        // Call dispatch to do the actual work
        let outcome = self.dispatch(node, state, user_message, mode, opts).await?;

        // Emit Final with reply and model
        let _ = events.send(DispatchEvent::Final {
            reply: outcome.reply.clone(),
            model: outcome.model.clone(),
        });

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_adapters::{AdapterError, AdapterResult};
    use n3ur0n_core::capability::CapabilityDecl;
    use serde_json::{Value, json};

    /// Mock backend for testing.
    #[derive(Debug, Clone)]
    struct MockBackend {
        response: String,
        model: String,
    }

    #[async_trait]
    impl Backend for MockBackend {
        async fn invoke(&self, capability: &str, _args: Value) -> AdapterResult<Value> {
            if capability != "chat" {
                return Err(AdapterError::UnknownCapability(capability.to_string()));
            }
            Ok(json!({
                "model": self.model,
                "message": {
                    "role": "assistant",
                    "content": self.response
                },
                "finish_reason": "stop"
            }))
        }

        async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> AdapterResult<n3ur0n_adapters::HealthStatus> {
            Ok(n3ur0n_adapters::HealthStatus::Healthy)
        }
    }

    /// Backend that tracks the model passed in invoke payload.
    #[derive(Debug, Clone)]
    struct ModelCheckBackend {
        expected_model: Option<String>,
    }

    #[async_trait]
    impl Backend for ModelCheckBackend {
        async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value> {
            if capability != "chat" {
                return Err(AdapterError::UnknownCapability(capability.to_string()));
            }

            // Verify model was passed correctly
            let model_in_payload = args.get("model").and_then(|v| v.as_str());
            match (&self.expected_model, model_in_payload) {
                (Some(exp), Some(actual)) => {
                    assert_eq!(exp, actual, "Model mismatch in payload");
                }
                (None, None) => {
                    // OK: both absent or null
                }
                _ => panic!("Model expectation mismatch"),
            }

            Ok(json!({
                "model": "test-model",
                "message": {
                    "role": "assistant",
                    "content": "Test response"
                },
                "finish_reason": "stop"
            }))
        }

        async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> AdapterResult<n3ur0n_adapters::HealthStatus> {
            Ok(n3ur0n_adapters::HealthStatus::Healthy)
        }
    }

    /// Backend that always returns an error.
    #[derive(Debug, Clone)]
    struct ErrorBackend;

    #[async_trait]
    impl Backend for ErrorBackend {
        async fn invoke(&self, _capability: &str, _args: Value) -> AdapterResult<Value> {
            Err(AdapterError::Backend(
                "simulated backend failure".to_string(),
            ))
        }

        async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> AdapterResult<n3ur0n_adapters::HealthStatus> {
            Ok(n3ur0n_adapters::HealthStatus::Healthy)
        }
    }

    /// Helper to create an in-memory test node.
    fn test_node() -> Node {
        use n3ur0n_core::identity::Keypair;
        use std::sync::Arc;
        use crate::registry::CapabilityRegistry;
        use crate::node::NodeConfig;
        use n3ur0n_adapters::echo::EchoBackend;

        let keypair = Keypair::generate();
        let db = n3ur0n_storage::open_in_memory().expect("Failed to create in-memory DB");
        let backend = Arc::new(EchoBackend);
        let registry = CapabilityRegistry::default();

        Node::new(keypair, db, backend, registry, NodeConfig::default())
    }

    #[tokio::test]
    async fn test_direct_dispatch_single_call() {
        let node = test_node();
        let backend = Arc::new(MockBackend {
            response: "Hello, this is a test response".to_string(),
            model: "test-llm".to_string(),
        });
        let planner = DirectChatPlanner::new(backend, None);

        let mut state = crate::conversation::create(
            node.db(),
            "client_1",
            Some("Test Conversation".to_string()),
        )
        .expect("Failed to create conversation");

        let outcome = planner
            .dispatch(
                &node,
                &mut state,
                "What is 2+2?".to_string(),
                DispatchMode::Direct,
                DispatchOptions::default(),
            )
            .await
            .expect("dispatch failed");

        // Verify outcome
        assert_eq!(outcome.reply, "Hello, this is a test response");
        assert_eq!(outcome.model, Some("test-llm".to_string()));
        assert_eq!(outcome.trace.len(), 0, "Direct mode should have empty trace");

        // Verify turns: should have User + Assistant (2 turns)
        assert_eq!(state.turns.len(), 2);
        match &state.turns[0] {
            crate::conversation::Turn::User { content, .. } => {
                assert_eq!(content, "What is 2+2?");
            }
            _ => panic!("First turn should be User"),
        }
        match &state.turns[1] {
            crate::conversation::Turn::Assistant { content, model, .. } => {
                assert_eq!(content, "Hello, this is a test response");
                assert_eq!(model, &Some("test-llm".to_string()));
            }
            _ => panic!("Second turn should be Assistant"),
        }
    }

    #[tokio::test]
    async fn test_direct_respects_model_override() {
        let node = test_node();
        let backend = Arc::new(ModelCheckBackend {
            expected_model: Some("custom-model".to_string()),
        });
        let planner = DirectChatPlanner::new(backend, Some("default-model".to_string()));

        let mut state =
            crate::conversation::create(node.db(), "client_1", None)
                .expect("Failed to create conversation");

        let outcome = planner
            .dispatch(
                &node,
                &mut state,
                "Hello".to_string(),
                DispatchMode::Direct,
                DispatchOptions {
                    model_override: Some("custom-model".to_string()),
                },
            )
            .await
            .expect("dispatch failed");

        // The mock verified the model was passed correctly
        assert_eq!(outcome.model, Some("test-model".to_string()));
    }

    #[tokio::test]
    async fn test_direct_error_propagates() {
        let node = test_node();
        let backend = Arc::new(ErrorBackend);
        let planner = DirectChatPlanner::new(backend, None);

        let mut state =
            crate::conversation::create(node.db(), "client_1", None)
                .expect("Failed to create conversation");

        let result = planner
            .dispatch(
                &node,
                &mut state,
                "Test message".to_string(),
                DispatchMode::Direct,
                DispatchOptions::default(),
            )
            .await;

        // Should fail
        assert!(result.is_err(), "Expected error from backend");

        // Should have only the User turn (Assistant was not pushed on error)
        assert_eq!(state.turns.len(), 1);
        match &state.turns[0] {
            crate::conversation::Turn::User { content, .. } => {
                assert_eq!(content, "Test message");
            }
            _ => panic!("Only turn should be User"),
        }
    }

    #[tokio::test]
    async fn test_direct_streaming_events() {
        let node = test_node();
        let backend = Arc::new(MockBackend {
            response: "Streamed response".to_string(),
            model: "stream-model".to_string(),
        });
        let planner = DirectChatPlanner::new(backend, None);

        let mut state =
            crate::conversation::create(node.db(), "client_1", None)
                .expect("Failed to create conversation");

        // Create event channel
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let outcome = planner
            .dispatch_streaming(
                &node,
                &mut state,
                "Stream test".to_string(),
                DispatchMode::Direct,
                DispatchOptions::default(),
                tx,
            )
            .await
            .expect("dispatch_streaming failed");

        // Collect events
        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Verify event sequence: PlanReady, Reflecting, Final
        assert_eq!(events.len(), 3, "Should have exactly 3 events");

        match &events[0] {
            DispatchEvent::PlanReady { steps } => {
                assert_eq!(steps.len(), 0, "Direct mode should have empty steps");
            }
            _ => panic!("First event should be PlanReady"),
        }

        match &events[1] {
            DispatchEvent::Reflecting => {}
            _ => panic!("Second event should be Reflecting"),
        }

        match &events[2] {
            DispatchEvent::Final { reply, model } => {
                assert_eq!(reply, "Streamed response");
                assert_eq!(model, &Some("stream-model".to_string()));
            }
            _ => panic!("Third event should be Final"),
        }

        // Verify outcome
        assert_eq!(outcome.reply, "Streamed response");
        assert_eq!(outcome.model, Some("stream-model".to_string()));
        assert_eq!(outcome.trace.len(), 0);
    }
}
