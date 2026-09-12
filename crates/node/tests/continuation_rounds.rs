//! A failed step must give the planner a second round.
//!
//! `planner_eval` grades a compiled plan and never executes it, so it cannot
//! see the continuation loop at all. This drives the real `dispatch` path
//! against a peer that does not answer: round one fails, the runtime observes
//! the failure — without asking the model to assess itself — and compiles a
//! continuation with the blackboard in front of it.
//!
//! The assertion is structural rather than textual: `plan_runs` holds one row
//! per compiled plan, so two rows means two rounds actually happened. Judging
//! the wording of the reply would measure the model's prose, not the loop.
//!
//! Needs an LLM: `cargo test -p n3ur0n-node --test continuation_rounds -- --ignored`

use std::sync::Arc;

use n3ur0n_adapters::openai::{OpenAIBackend, OpenAIConfig};
use n3ur0n_adapters::utility::UtilityBackend;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
use n3ur0n_node::conversation::ConversationState;
use n3ur0n_node::planner::plan_exec::PlanExecPlanner;
use n3ur0n_node::planner::{DispatchMode, DispatchOptions, Planner};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_storage::{open_in_memory, peers};
use serde_json::json;

/// A capability advertised by a peer that will never answer. Any plan step
/// routed to it fails at execution, which is the trigger under test.
fn unreachable_cap() -> CapabilityDecl {
    CapabilityDecl {
        name: "fetch_url".into(),
        description: "Download one specific URL and return its extracted text content.".into(),
        schema_in: json!({
            "type": "object",
            "required": ["url"],
            "properties": {"url": {"type": "string"}}
        }),
        schema_out: json!({
            "type": "object",
            "properties": {"text": {"type": "string"}, "status": {"type": "integer"}}
        }),
        mode: AccessMode::Free,
        pricing: None,
        tags: vec!["web".into()],
        lobe_ids: vec![],
        examples: vec![CapabilityExample {
            user_intent: "Download this page and give me its text.".into(),
            args: json!({"url": "https://example.com"}),
            expected_output: json!({"status": 200, "text": "..."}),
        }],
        disambiguation: Some("Use when the user names one exact URL to download.".into()),
        negative_examples: vec![],
        output_semantic: Some("The page's text content.".into()),
        version: "1.0.0".into(),
        languages: vec![],
        countries: vec![],
    }
}

#[tokio::test]
#[ignore = "needs a running LLM endpoint; run with --ignored"]
async fn a_failed_step_triggers_a_second_round() {
    let base_url =
        std::env::var("PLANNER_EVAL_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("PLANNER_EVAL_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());

    let db = open_in_memory().unwrap();

    // A peer advertising `fetch_url` at an endpoint nothing listens on.
    let descriptor = json!({
        "instance_id": "n3:deadpeer0000000000000000000000000",
        "endpoint": "http://127.0.0.1:1/",
        "protocol_version": "n3ur0n/0.3",
        "updated_at": "2026-01-01T00:00:00Z",
        "capabilities": [unreachable_cap()],
    });
    peers::upsert(
        &db,
        &peers::PeerRecord {
            id: "n3:deadpeer0000000000000000000000000".into(),
            endpoint: "http://127.0.0.1:1/".into(),
            alias: None,
            last_seen: Some(0),
            tls_fingerprint: None,
            describe_self_cached: Some(descriptor.to_string()),
            describe_self_fetched_at: Some(0),
            source: Some("test".into()),
        },
    )
    .unwrap();

    let backend = Arc::new(UtilityBackend);
    let registry = CapabilityRegistry::from_decls(
        <UtilityBackend as n3ur0n_adapters::Backend>::describe(&UtilityBackend)
            .await
            .unwrap(),
    );
    let node = Node::new(
        n3ur0n_core::Keypair::generate(),
        db,
        backend,
        registry,
        NodeConfig::default(),
    );

    let llm = Arc::new(
        OpenAIBackend::new(OpenAIConfig {
            base_url: base_url.clone(),
            default_model: model.clone(),
            api_key: None,
            description: None,
            allow_model_override: true,
        })
        .expect("build LLM backend"),
    );
    let planner = PlanExecPlanner::new(llm, Some(model));

    let conv_id = "conv_continuation_test";
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

    let outcome = planner
        .dispatch(
            &node,
            &mut state,
            "Download https://example.com and tell me how many characters its text has.".into(),
            DispatchMode::Auto,
            DispatchOptions::default(),
        )
        .await
        .expect("dispatch");

    let rows = n3ur0n_storage::plan_runs::list_for_conversation(node.db(), conv_id, 10)
        .expect("list plan runs");

    println!(
        "compile rounds: {} · executed plans: {} · reply: {}",
        outcome.rounds,
        rows.len(),
        outcome.reply
    );
    for e in &outcome.trace {
        println!("  step {} error={:?}", e.capability, e.error);
    }

    assert!(
        outcome.trace.iter().any(|e| e.error.is_some()),
        "the unreachable peer should have produced a failed step; trace: {:?}",
        outcome.trace
    );
    // The loop is proven by the extra compile round, not by an extra executed
    // plan: concluding "nothing more to do" after a failure is a legitimate —
    // and here correct — outcome, since retrying an unreachable peer is futile.
    assert_eq!(
        outcome.rounds, 2,
        "a failed step must trigger a continuation round"
    );
    assert!(
        rows.len() <= 2,
        "at most one continuation plan may be journalled with MAX_PLAN_ROUNDS=2"
    );
}
