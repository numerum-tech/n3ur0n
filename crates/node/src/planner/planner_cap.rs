//! Expose a [`PlanCompiler`] as the `plan` capability on the network.
//!
//! A publisher can wrap any `PlanCompiler` (typically a `LocalLLMCompiler`
//! backed by a larger model than the caller can run locally) and serve it
//! to peers that bootstrap with `N3UR0N_PLANNER_REMOTE_FALLBACK`. The peer
//! sends `{user_intent, catalog}` and receives `{plan: [...]}`.
//!
//! Trust model (v0.2): the catalog is taken from the caller's payload, not
//! the publisher's own peer directory. This lets the remote planner reason
//! about the *caller's* network view. Implicitly trusts the caller's
//! catalog declarations — fine for a LAN-trusted deployment; tighten in
//! v0.3 if/when caps gain economic effect.
//!
//! `EXCLUDED_CAP_NAMES` in `catalog.rs` still hides the local `plan` cap
//! from any planner running on this node, preventing plan→plan recursion.

use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::{AdapterError, AdapterResult, Backend, HealthStatus};
use n3ur0n_core::capability::{
    AccessMode, CapabilityDecl, CapabilityExample, NegativeExample,
};
use serde_json::{json, Value};

use crate::planner::catalog::{Catalog, ToolDef};
use crate::planner::compiler::PlanCompiler;

const CAP_NAME: &str = "plan";

#[derive(Clone)]
pub struct PlannerAsCapability {
    pub compiler: Arc<dyn PlanCompiler>,
}

impl std::fmt::Debug for PlannerAsCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlannerAsCapability")
            .field("compiler", &self.compiler)
            .finish()
    }
}

impl PlannerAsCapability {
    pub fn new(compiler: Arc<dyn PlanCompiler>) -> Self {
        Self { compiler }
    }
}

#[async_trait]
impl Backend for PlannerAsCapability {
    async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value> {
        if capability != CAP_NAME {
            return Err(AdapterError::UnknownCapability(capability.to_string()));
        }

        let user_intent = args
            .get("user_intent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::Backend("missing `user_intent` (string)".into()))?
            .to_string();

        let catalog: Catalog = parse_catalog(args.get("catalog"))
            .map_err(|e| AdapterError::Backend(format!("invalid catalog: {e}")))?;

        let plan = self
            .compiler
            .compile(&user_intent, &catalog)
            .await
            .map_err(|e| AdapterError::Backend(format!("compile: {e}")))?;

        // Return {plan: [...]} so the cap output matches what the
        // RemotePlanCompiler expects to deserialise.
        let body = serde_json::to_value(&plan)
            .map_err(AdapterError::Serde)?;
        Ok(body)
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        Ok(vec![CapabilityDecl {
            name: CAP_NAME.into(),
            description: "Compile a typed Plan from a user intent and a serialised \
catalog of tools. Used by peers that delegate planning to a node with a stronger \
model."
                .into(),
            schema_in: plan_cap_schema_in(),
            schema_out: plan_cap_schema_out(),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec!["planner".into(), "meta".into()],
            lobe_ids: vec![],
            examples: vec![
                CapabilityExample {
                    user_intent: "delegate a translation task".into(),
                    args: json!({
                        "user_intent": "Translate 'hello' to French.",
                        "catalog": []
                    }),
                    expected_output: json!({"plan": []}),
                },
                CapabilityExample {
                    user_intent: "delegate a multi-step utility chain".into(),
                    args: json!({
                        "user_intent": "Generate a random integer and reverse its digits.",
                        "catalog": [
                            {"peer_id":"n3:abc","peer_endpoint":"http://x","cap":{
                                "name":"random_int","description":"random int","schema_in":{},"schema_out":{},"mode":"free","tags":[],"lobe_ids":[],
                                "examples":[{"user_intent":"pick","args":{},"expected_output":{}}]
                            }}
                        ]
                    }),
                    expected_output: json!({
                        "plan": [
                            {"id":"s1","peer":"abc","capability":"random_int","args":{}}
                        ]
                    }),
                },
            ],
            disambiguation: Some(
                "Meta-cap: produces a plan others execute. Not for direct user-facing \
content generation — pair with a `chat` cap on the caller side for replies."
                    .into(),
            ),
            negative_examples: vec![NegativeExample {
                user_intent: "answer a question directly".into(),
                why_not: "plan emits a plan, not a reply; use a chat cap for direct \
answers."
                    .into(),
            }],
            output_semantic: Some("Typed Plan ready for a deterministic executor.".into()),
        }])
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }
}

fn plan_cap_schema_in() -> Value {
    json!({
        "type": "object",
        "required": ["user_intent"],
        "properties": {
            "user_intent": {"type": "string"},
            "catalog": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["peer_id", "cap"],
                    "properties": {
                        "peer_id": {"type": "string"},
                        "peer_endpoint": {"type": ["string", "null"]},
                        "cap": {"type": "object"}
                    }
                }
            }
        }
    })
}

fn plan_cap_schema_out() -> Value {
    json!({
        "type": "object",
        "required": ["plan"],
        "properties": {
            "plan": {
                "type": "array",
                "items": {"type": "object"}
            }
        }
    })
}

fn parse_catalog(value: Option<&Value>) -> Result<Catalog, String> {
    let Some(value) = value else {
        return Ok(Catalog::default());
    };
    let arr = value
        .as_array()
        .ok_or_else(|| "`catalog` must be an array".to_string())?;
    let mut tools = Vec::with_capacity(arr.len());
    for item in arr {
        let peer_id = item
            .get("peer_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "tool entry missing `peer_id`".to_string())?
            .to_string();
        let peer_endpoint = item
            .get("peer_endpoint")
            .and_then(|v| v.as_str())
            .map(String::from);
        let cap_value = item
            .get("cap")
            .ok_or_else(|| "tool entry missing `cap`".to_string())?
            .clone();
        let cap: CapabilityDecl = serde_json::from_value(cap_value)
            .map_err(|e| format!("invalid `cap`: {e}"))?;
        tools.push(ToolDef {
            peer_id,
            peer_endpoint,
            cap,
        });
    }
    Ok(Catalog { tools })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_catalog_empty_or_missing() {
        assert_eq!(parse_catalog(None).unwrap().tools.len(), 0);
        let empty = json!([]);
        assert_eq!(parse_catalog(Some(&empty)).unwrap().tools.len(), 0);
    }

    #[test]
    fn parse_catalog_round_trip() {
        let value = json!([
            {
                "peer_id": "n3:p1",
                "peer_endpoint": "http://p1:4242",
                "cap": {
                    "name": "noop",
                    "description": "d",
                    "schema_in": {},
                    "schema_out": {},
                    "mode": "free",
                    "tags": [],
                    "lobe_ids": [],
                    "examples": [{"user_intent":"go","args":{},"expected_output":{}}]
                }
            }
        ]);
        let cat = parse_catalog(Some(&value)).unwrap();
        assert_eq!(cat.tools.len(), 1);
        assert_eq!(cat.tools[0].cap.name, "noop");
    }
}
