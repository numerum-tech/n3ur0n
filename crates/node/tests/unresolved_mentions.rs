//! Addressing something the node does not know refuses the request.
//!
//! The alternative, which this replaces, was to answer anyway against the full
//! catalogue and ask the reflect model to disclose the miss in prose. A 7B
//! model drops that instruction, and the user then believes a request was
//! scoped to one peer when another served it. Resolution is deterministic and
//! happens before any LLM call, so the refusal can be too.

use std::sync::Arc;

use n3ur0n_adapters::Backend;
use n3ur0n_adapters::utility::UtilityBackend;
use n3ur0n_node::conversation::{ConversationState, UserInput};
use n3ur0n_node::planner::{DispatchMode, DispatchOptions, PlanExecPlanner, Planner};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig, NodeError};
use n3ur0n_storage::open_in_memory;

/// An LLM endpoint that would fail loudly if it were ever called: the point of
/// the test is that nothing reaches it.
fn unreachable_llm() -> Arc<n3ur0n_adapters::openai::OpenAIBackend> {
    Arc::new(
        n3ur0n_adapters::openai::OpenAIBackend::new(n3ur0n_adapters::openai::OpenAIConfig {
            base_url: "http://127.0.0.1:1".into(),
            default_model: "unused".into(),
            api_key: None,
            description: None,
            allow_model_override: true,
        })
        .expect("build LLM backend"),
    )
}

async fn node_with_utility_caps() -> Node {
    let backend = Arc::new(UtilityBackend);
    let decls = UtilityBackend.describe().await.unwrap();
    Node::new(
        n3ur0n_core::Keypair::generate(),
        open_in_memory().unwrap(),
        backend,
        CapabilityRegistry::from_decls(decls),
        NodeConfig::default(),
    )
}

#[tokio::test]
async fn an_unknown_peer_refuses_the_dispatch_and_names_the_token() {
    let node = node_with_utility_caps().await;
    let planner = PlanExecPlanner::new(unreachable_llm(), Some("unused".into()));
    let mut state = ConversationState::new("conv-unresolved".into(), "client-test".into(), None);

    let err = planner
        .dispatch(
            &node,
            &mut state,
            UserInput::from("@peer:seed#4hzrfsedcr reverse the text n3ur0n".to_string()),
            DispatchMode::Auto,
            DispatchOptions::default(),
        )
        .await
        .expect_err("an unknown peer must refuse, not answer against a wider scope");

    match err {
        NodeError::UnresolvedMentions(tokens) => {
            assert_eq!(tokens, vec!["@peer:seed#4hzrfsedcr".to_string()]);
        }
        other => panic!("expected UnresolvedMentions, got {other:?}"),
    }

    // Refused means refused: no turn was recorded, so the thread does not keep
    // a user message with nothing under it.
    assert!(state.turns.is_empty(), "a refused command records nothing");
}

#[tokio::test]
async fn a_known_capability_still_dispatches() {
    let node = node_with_utility_caps().await;
    let planner = PlanExecPlanner::new(unreachable_llm(), Some("unused".into()));
    let mut state = ConversationState::new("conv-known".into(), "client-test".into(), None);

    // `@cap:reverse` resolves, so the refusal must not fire. The dispatch then
    // fails on the unreachable LLM, which is a different error entirely — that
    // is the distinction being asserted.
    let err = planner
        .dispatch(
            &node,
            &mut state,
            UserInput::from("@cap:reverse reverse the text n3ur0n".to_string()),
            DispatchMode::Auto,
            DispatchOptions::default(),
        )
        .await;
    if let Err(NodeError::UnresolvedMentions(tokens)) = err {
        panic!("a known capability must not be reported as unresolved: {tokens:?}");
    }
}
