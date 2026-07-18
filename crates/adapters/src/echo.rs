//! Echo backend: returns the args verbatim. For dev / smoke tests.

use async_trait::async_trait;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample, NegativeExample};
use serde_json::{Value, json};

use crate::{AdapterResult, Backend, HealthStatus};

#[derive(Debug, Default, Clone, Copy)]
pub struct EchoBackend;

#[async_trait]
impl Backend for EchoBackend {
    async fn invoke(&self, _capability: &str, args: Value) -> AdapterResult<Value> {
        Ok(args)
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        Ok(vec![CapabilityDecl {
            name: "echo".into(),
            description: "Returns its input arguments untouched. Useful as a probe \
or smoke test; provides no transformation."
                .into(),
            schema_in: json!({"type": "object"}),
            schema_out: json!({"type": "object"}),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec!["debug".into(), "smoke".into()],
            lobe_ids: vec![],
            examples: vec![CapabilityExample {
                user_intent: "round-trip a payload to verify reachability".into(),
                args: json!({"hello": "world"}),
                expected_output: json!({"hello": "world"}),
            }],
            disambiguation: Some(
                "Diagnostic cap only. Never add as an intermediate step in a multi-step \
plan — it produces no useful data for downstream consumers."
                    .into(),
            ),
            negative_examples: vec![
                NegativeExample {
                    user_intent: "transform or format the user input".into(),
                    why_not: "echo does not modify its input; pick a transformation cap \
(e.g. `reverse`, `string_length`, or a chat cap) instead."
                        .into(),
                },
                NegativeExample {
                    user_intent: "relay an earlier step's result so it appears in the trace".into(),
                    why_not: "the executor's blackboard already carries every step's \
result; an echo step is redundant filler."
                        .into(),
                },
            ],
            output_semantic: Some("Same JSON object the caller supplied.".into()),
            version: "0.1.0".into(),
            languages: vec![],
            countries: vec![],
        }])
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }
}
