//! The reflect prompt must state the user's request exactly once.
//!
//! `dispatch_inner` records the user turn before compiling, so the
//! conversation tail handed to reflect already ends on it. Re-stating it
//! afterwards — which reflect does on purpose, to put the question after the
//! blackboard — used to leave two identical consecutive `user` messages in the
//! prompt. A model reads that as two requests and answers both: qwen2.5:7b
//! returns "Hi to you!\n\nHi to you!" to a greeting, and "I tried ... twice,
//! and it failed both times" when a step failed.
//!
//! The assertion is on the prompt, not the reply: what the model does with a
//! doubled question is the model's business, sending it twice is ours. A
//! recording backend keeps this test free of any LLM.

mod common;

use std::sync::{Arc, Mutex};

use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_adapters::{AdapterResult, Backend, HealthStatus};
use n3ur0n_core::capability::CapabilityDecl;
use n3ur0n_node::conversation::ConversationState;
use n3ur0n_node::planner::plan_exec::PlanExecPlanner;
use n3ur0n_node::planner::{DispatchMode, DispatchOptions, Planner};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_storage::open_in_memory;
use serde_json::{Value, json};

/// Records every `messages` array it is asked to complete, and always compiles
/// the empty plan — the shortest path to reflect.
#[derive(Debug, Default)]
struct RecordingLlm {
    calls: Mutex<Vec<Vec<Value>>>,
}

#[async_trait::async_trait]
impl Backend for RecordingLlm {
    async fn invoke(&self, _capability: &str, args: Value) -> AdapterResult<Value> {
        let messages = args
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let is_compile = messages
            .first()
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("plan compiler"));
        self.calls.lock().unwrap().push(messages);
        let content = if is_compile { r#"{"plan": []}"# } else { "ok" };
        Ok(json!({
            "model": "recording",
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }))
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        Ok(Vec::new())
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }
}

/// The `user` messages of the reflect call, in order.
fn reflect_user_messages(calls: &[Vec<Value>]) -> Vec<String> {
    let reflect = calls
        .iter()
        .find(|ms| {
            ms.first()
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains("response composer"))
        })
        .expect("reflect call was made");
    reflect
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn dispatch_and_capture(message: &str) -> Vec<Vec<Value>> {
    let db = open_in_memory().unwrap();
    let node = Node::new(
        n3ur0n_core::Keypair::generate(),
        db,
        Arc::new(EchoBackend),
        CapabilityRegistry::from_decls(common::cluster_cap_decls()),
        NodeConfig::default(),
    );

    let conv_id = "conv_reflect_prompt";
    n3ur0n_storage::conversations::insert(
        node.db(),
        &n3ur0n_storage::conversations::ConversationRecord {
            id: conv_id.into(),
            client_id: "client-test".into(),
            title: None,
            created_at: 0,
            updated_at: 0,
        },
    )
    .unwrap();
    let mut state = ConversationState::new(conv_id.into(), "client-test".into(), None);

    let llm = Arc::new(RecordingLlm::default());
    let planner = PlanExecPlanner::new(llm.clone(), Some("recording".into()));
    planner
        .dispatch(
            &node,
            &mut state,
            message.into(),
            DispatchMode::Auto,
            DispatchOptions::default(),
        )
        .await
        .expect("dispatch");

    let calls = llm.calls.lock().unwrap();
    calls.clone()
}

#[tokio::test]
async fn reflect_states_the_request_once() {
    let calls = dispatch_and_capture("Hi to you!").await;
    let users = reflect_user_messages(&calls);
    assert_eq!(
        users,
        vec!["Hi to you!".to_string()],
        "the request must appear once in the reflect prompt, not once from the \
         conversation tail and once from the re-statement"
    );
}

#[tokio::test]
async fn reflect_keeps_earlier_turns_and_ends_on_the_request() {
    let db = open_in_memory().unwrap();
    let node = Node::new(
        n3ur0n_core::Keypair::generate(),
        db,
        Arc::new(EchoBackend),
        CapabilityRegistry::from_decls(common::cluster_cap_decls()),
        NodeConfig::default(),
    );
    let conv_id = "conv_reflect_prompt_history";
    n3ur0n_storage::conversations::insert(
        node.db(),
        &n3ur0n_storage::conversations::ConversationRecord {
            id: conv_id.into(),
            client_id: "client-test".into(),
            title: None,
            created_at: 0,
            updated_at: 0,
        },
    )
    .unwrap();
    let mut state = ConversationState::new(conv_id.into(), "client-test".into(), None);
    // An earlier, completed exchange: only the turn being answered is dropped.
    state.push_user("first question");
    state.push_assistant("first answer".into(), None);

    let llm = Arc::new(RecordingLlm::default());
    let planner = PlanExecPlanner::new(llm.clone(), Some("recording".into()));
    planner
        .dispatch(
            &node,
            &mut state,
            "second question".into(),
            DispatchMode::Auto,
            DispatchOptions::default(),
        )
        .await
        .expect("dispatch");

    let calls = llm.calls.lock().unwrap().clone();
    let users = reflect_user_messages(&calls);
    assert_eq!(
        users,
        vec!["first question".to_string(), "second question".to_string()],
        "history stays; only the duplicate of the current turn goes"
    );
    let reflect = calls
        .iter()
        .find(|ms| {
            ms.first()
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains("response composer"))
        })
        .unwrap();
    assert_eq!(
        reflect.last().unwrap()["content"], "second question",
        "the request is the last thing the model reads"
    );
}
