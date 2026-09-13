//! A local plan step must run the capability's own binding.
//!
//! `handler.rs` has always preferred the registry binding over the
//! compile-time backend for an inbound invoke. The plan executor did not: it
//! called `node.backend()` for any step with no endpoint. In manifest mode
//! that slot holds an inert `EchoBackend`, so a node executing *its own*
//! capability got its arguments echoed back as the result — `time` answered
//! `{}` and the planner reported it could not read the clock, while the same
//! capability answered correctly over a signed invoke from a peer.
//!
//! No LLM: the plan is handed to the executor directly.

use std::sync::Arc;

use n3ur0n_adapters::echo::EchoBackend;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
use n3ur0n_node::bindings::Binding;
use n3ur0n_node::error::NodeResult;
use n3ur0n_node::planner::catalog::Catalog;
use n3ur0n_node::planner::plan::{Plan, PlanStep, execute_plan};
use n3ur0n_node::{CapabilityRegistry, Node, NodeConfig};
use n3ur0n_storage::open_in_memory;
use serde_json::{Value, json};

/// Answers something the arguments do not contain, so an echo cannot be
/// mistaken for a real call.
#[derive(Debug)]
struct ClockBinding;

#[async_trait::async_trait]
impl Binding for ClockBinding {
    async fn invoke(&self, _args: Value) -> NodeResult<Value> {
        Ok(json!({"now": "2026-09-13T00:00:00Z", "unix": 1789243200}))
    }
    fn kind(&self) -> &'static str {
        "http"
    }
}

fn clock_decl() -> CapabilityDecl {
    CapabilityDecl {
        name: "time".into(),
        description: "Current server time.".into(),
        schema_in: json!({"type": "object"}),
        schema_out: json!({
            "type": "object",
            "required": ["now", "unix"],
            "properties": {"now": {"type": "string"}, "unix": {"type": "integer"}}
        }),
        mode: AccessMode::Free,
        pricing: None,
        tags: vec![],
        lobe_ids: vec![],
        // A local cap with no example is dropped from the planner catalogue.
        examples: vec![CapabilityExample {
            user_intent: "what time is it".into(),
            args: json!({}),
            expected_output: json!({"now": "2026-01-01T00:00:00Z", "unix": 1767225600}),
        }],
        disambiguation: None,
        negative_examples: vec![],
        output_semantic: None,
        version: "0.1.0".into(),
        languages: vec![],
        countries: vec![],
    }
}

#[tokio::test]
async fn a_local_step_runs_the_binding_not_the_inert_backend() {
    let db = open_in_memory().unwrap();
    let registry = CapabilityRegistry::from_entries(vec![(
        clock_decl(),
        Arc::new(ClockBinding) as Arc<dyn Binding>,
    )]);
    // Exactly the manifest-mode shape: the compile-time slot is inert.
    let node = Node::new(
        n3ur0n_core::Keypair::generate(),
        db,
        Arc::new(EchoBackend),
        registry,
        NodeConfig::default(),
    );

    let catalog = Catalog::build(node.instance_id().as_str(), &node.registry(), node.db(), 50)
        .expect("build catalog");
    // Address the step the way a compiled plan does: with the short peer the
    // catalogue itself advertises.
    let tool = catalog
        .tools
        .iter()
        .find(|t| t.cap.name == "time")
        .expect("the local cap is in the catalogue");
    let tool_name = catalog.tool_name(tool);
    let (peer_short, _) = tool_name.split_once("::").expect("peer::cap");
    let plan = Plan {
        plan: vec![PlanStep {
            id: "s1".into(),
            peer: peer_short.to_string(),
            capability: "time".into(),
            args: json!({}),
            depends_on: vec![],
        }],
    };

    let run = execute_plan(&node, &plan, &catalog).await.expect("execute");
    let entry = run.trace.first().expect("one step ran");
    assert_eq!(entry.error, None, "step failed: {:?}", entry.error);
    assert_eq!(
        entry.result.as_ref().and_then(|v| v.get("now")),
        Some(&json!("2026-09-13T00:00:00Z")),
        "the binding's answer must reach the blackboard, not the echoed args"
    );
}
