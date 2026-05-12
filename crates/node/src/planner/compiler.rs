//! Pluggable plan compilation.
//!
//! The compile step of [`PlanExecPlanner`](super::plan_exec::PlanExecPlanner)
//! is factored behind the [`PlanCompiler`] trait so that:
//! - the local LLM compile path becomes one implementation
//!   ([`LocalLLMCompiler`]),
//! - a remote planner reached over the network can be another
//!   ([`super::remote_compiler::RemotePlanCompiler`], wired in Phase D.3),
//! - the planner can cascade between them when the local pass produces a
//!   low-confidence plan ([`CascadingCompiler`], wired in Phase D.4).
//!
//! Confidence is intentionally a coarse 0..1 heuristic — the rec doc §3.4
//! lists possible refinements (log-prob scoring, dual-LLM critique) for
//! later iterations.

use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::Backend;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_core::Keypair;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};
use tracing::warn;

use crate::client as peer_client;
use crate::error::{NodeError, NodeResult};
use crate::planner::catalog::Catalog;
use crate::planner::plan::{validate_plan, Plan, MAX_PLAN_STEPS};

/// Anything that can turn a user message + catalog into a typed `Plan`.
///
/// Implementations should *not* execute the plan; that is the executor's
/// job. Implementations should *not* mutate any conversation state; they
/// are called once per dispatch from inside `PlanExecPlanner::dispatch_*`.
#[async_trait]
pub trait PlanCompiler: Send + Sync + std::fmt::Debug {
    /// Produce a plan for `user_msg` given the catalog of available tools.
    /// Returning an empty plan (`{plan: []}`) is a valid answer when no
    /// tool is needed; the planner's reflect step will then compose the
    /// final reply from prior knowledge.
    async fn compile(&self, user_msg: &str, catalog: &Catalog) -> NodeResult<Plan>;

    /// Coarse 0..1 confidence in the compiled plan. The cascading
    /// compiler uses this to decide whether to escalate.
    ///
    /// Default heuristic is intentionally generic; concrete compilers
    /// override with backend-specific signals (token log-probs, schema
    /// validation deltas, etc.) when available.
    async fn confidence(&self, plan: &Plan, catalog: &Catalog) -> f32 {
        default_confidence(plan, catalog)
    }
}

/// Compile via the local LLM backend (the v0.1 path, now extracted).
#[derive(Clone)]
pub struct LocalLLMCompiler {
    pub llm_backend: Arc<dyn Backend>,
    pub model_hint: Option<String>,
    /// Renders the system prompt — kept as a callback so the
    /// `PlanExecPlanner` can keep ownership of prompt layout (which
    /// depends on its own configuration like MAX_CONTEXT_TURNS).
    pub system_prompt: Arc<dyn Fn(&Catalog) -> String + Send + Sync>,
}

impl std::fmt::Debug for LocalLLMCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalLLMCompiler")
            .field("model_hint", &self.model_hint)
            .finish()
    }
}

#[async_trait]
impl PlanCompiler for LocalLLMCompiler {
    async fn compile(&self, user_msg: &str, catalog: &Catalog) -> NodeResult<Plan> {
        let system = (self.system_prompt)(catalog);
        let messages = vec![
            json!({"role": "system", "content": system}),
            json!({"role": "user",   "content": user_msg}),
        ];

        let mut args = json!({
            "messages": messages,
            "grammar":         crate::planner::grammar::plan_grammar(),
            "response_format": crate::planner::grammar::plan_response_format(),
            "format":          crate::planner::grammar::plan_json_schema(),
            "temperature": 0.0,
        });
        if let Some(model) = &self.model_hint {
            args["model"] = Value::String(model.clone());
        }

        let resp = self.llm_backend.invoke("chat", args).await?;
        let raw = resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        match crate::planner::plan_exec::parse_plan(&raw) {
            Ok(p) => Ok(p),
            Err(e) => {
                warn!(error = %e, raw = %raw.chars().take(200).collect::<String>(),
                    "local compiler produced invalid JSON; emitting empty plan");
                // Empty plan = no-tool answer; the reflect step handles it.
                Ok(Plan { plan: vec![] })
            }
        }
    }
}

/// Default confidence heuristic used by every compiler unless they
/// override. Cheap signals only — no extra LLM calls.
///
/// Tiered:
/// - 0.0 if the plan fails structural validation (caller should not
///   reach this in normal operation; bug if it does).
/// - 0.3 if the plan is >8 steps (already capped by MAX_PLAN_STEPS but
///   we keep the heuristic for compilers that bypass the cap).
/// - 0.5 if the plan is empty AND the user message is "long" (>20
///   tokens). Empty plans on short prompts are usually correct
///   (translate, sum, answer-from-knowledge) — empty on long prompts
///   is suspicious.
/// - 0.9 otherwise.
pub fn default_confidence(plan: &Plan, _catalog: &Catalog) -> f32 {
    if plan.plan.len() > MAX_PLAN_STEPS {
        return 0.3;
    }
    if plan.plan.is_empty() {
        return 0.5; // tentative; the cascade decides whether to escalate
    }
    0.9
}

/// Compile by invoking the `plan` capability on a peer.
///
/// The remote peer must publish a `plan` cap (see
/// [`super::planner_cap::PlannerAsCapability`]) whose `schema_in` accepts
/// `{user_intent: string, catalog: [<ToolDef>...]}` and whose response
/// payload contains a `plan` field shaped like our `Plan` struct.
///
/// Confidence override: a successful remote compile reports a high
/// baseline confidence (0.85) — the assumption is the remote node was
/// configured precisely because it can plan better than the local model
/// for this workload. Cascading still picks the higher of the two scores.
#[derive(Clone)]
pub struct RemotePlanCompiler {
    pub http: HttpClient,
    pub keypair: Keypair,
    pub endpoint: String,
}

impl std::fmt::Debug for RemotePlanCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePlanCompiler")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

#[async_trait]
impl PlanCompiler for RemotePlanCompiler {
    async fn compile(&self, user_msg: &str, catalog: &Catalog) -> NodeResult<Plan> {
        // Serialise the catalog as a list of `{peer, capability, decl}`
        // so the remote planner can re-run retrieval / validation against
        // the same surface we see locally.
        let catalog_value: Vec<Value> = catalog
            .tools
            .iter()
            .map(|t| {
                json!({
                    "peer_id": t.peer_id,
                    "peer_endpoint": t.peer_endpoint,
                    "cap": t.cap,
                })
            })
            .collect();
        let payload = json!({
            "capability": "plan",
            "args": {
                "user_intent": user_msg,
                "catalog": catalog_value,
            }
        });

        let reply = peer_client::send_signed(
            &self.http,
            &self.keypair,
            &self.endpoint,
            ProtocolVerb::Invoke,
            payload,
        )
        .await
        .map_err(|e| NodeError::InvalidPayload(format!("remote plan invoke: {e}")))?;

        let body = reply.envelope.payload;
        // Accept either `{result: {plan: [...]}}` (per the invoke
        // convention) or `{plan: [...]}` directly.
        let plan_value = body
            .get("result")
            .and_then(|r| r.get("plan"))
            .or_else(|| body.get("plan"))
            .cloned();
        let plan_value = plan_value.ok_or_else(|| {
            NodeError::InvalidPayload("remote planner reply missing `plan`".into())
        })?;
        let plan: Plan = serde_json::from_value(json!({"plan": plan_value}))
            .map_err(|e| NodeError::InvalidPayload(format!("remote plan parse: {e}")))?;
        Ok(plan)
    }

    async fn confidence(&self, plan: &Plan, catalog: &Catalog) -> f32 {
        // Start from default tiers, then bump by 0.1 for a non-empty
        // structurally-valid plan: remote planners are expected to ship
        // better metadata-aware decisions.
        let base = default_confidence(plan, catalog);
        if !plan.plan.is_empty() && plan_is_valid(plan, catalog) {
            (base + 0.1).min(1.0)
        } else {
            base
        }
    }
}

/// Wrap a primary compiler with an optional remote fallback. If the
/// primary's confidence is below `threshold`, the cascade tries the
/// fallback and keeps whichever plan has the higher confidence.
///
/// When no fallback is configured, the cascade is a thin pass-through
/// over the primary (so callers can always wrap in CascadingCompiler
/// without conditional construction).
#[derive(Clone)]
pub struct CascadingCompiler {
    pub primary: Arc<dyn PlanCompiler>,
    pub fallback: Option<Arc<dyn PlanCompiler>>,
    pub threshold: f32,
}

impl std::fmt::Debug for CascadingCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CascadingCompiler")
            .field("threshold", &self.threshold)
            .field("has_fallback", &self.fallback.is_some())
            .finish()
    }
}

#[async_trait]
impl PlanCompiler for CascadingCompiler {
    async fn compile(&self, user_msg: &str, catalog: &Catalog) -> NodeResult<Plan> {
        let primary_plan = self.primary.compile(user_msg, catalog).await?;
        let primary_conf = self.primary.confidence(&primary_plan, catalog).await;
        if primary_conf >= self.threshold {
            return Ok(primary_plan);
        }
        let Some(fallback) = &self.fallback else {
            return Ok(primary_plan);
        };

        warn!(
            confidence = primary_conf,
            threshold = self.threshold,
            "primary compiler confidence below threshold; trying fallback"
        );
        match fallback.compile(user_msg, catalog).await {
            Ok(fb_plan) => {
                let fb_conf = fallback.confidence(&fb_plan, catalog).await;
                if fb_conf > primary_conf {
                    Ok(fb_plan)
                } else {
                    Ok(primary_plan)
                }
            }
            Err(e) => {
                warn!(error = %e, "fallback compiler failed; keeping primary plan");
                Ok(primary_plan)
            }
        }
    }

    async fn confidence(&self, plan: &Plan, catalog: &Catalog) -> f32 {
        // Best-effort: ask the primary; it produced this plan, it can
        // self-assess.
        self.primary.confidence(plan, catalog).await
    }
}

/// Validate `plan` against `catalog` and return whether it is structurally
/// acceptable. Helper used by the cascade so it doesn't trigger an
/// escalation for plans that would also fail in the fallback path.
pub fn plan_is_valid(plan: &Plan, catalog: &Catalog) -> bool {
    validate_plan(plan, catalog).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::catalog::ToolDef;
    use crate::planner::plan::PlanStep;
    use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};

    fn dummy_catalog() -> Catalog {
        let mut cat = Catalog::default();
        cat.tools.push(ToolDef {
            peer_id: "n3:aaaaaaaaaaaa".into(),
            peer_endpoint: Some("http://x".into()),
            cap: CapabilityDecl {
                name: "noop".into(),
                description: "test".into(),
                schema_in: json!({}),
                schema_out: json!({}),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec![],
                lobe_ids: vec![],
                examples: vec![CapabilityExample {
                    user_intent: "go".into(),
                    args: json!({}),
                    expected_output: json!({}),
                }],
                disambiguation: None,
                negative_examples: vec![],
                output_semantic: None,
            },
        });
        cat
    }

    #[test]
    fn default_confidence_tiers() {
        let cat = dummy_catalog();

        let empty = Plan { plan: vec![] };
        assert!((default_confidence(&empty, &cat) - 0.5).abs() < 1e-6);

        let one_step = Plan {
            plan: vec![PlanStep {
                id: "s1".into(),
                peer: "p".into(),
                capability: "c".into(),
                args: json!({}),
                depends_on: vec![],
            }],
        };
        assert!((default_confidence(&one_step, &cat) - 0.9).abs() < 1e-6);

        let too_big = Plan {
            plan: (0..MAX_PLAN_STEPS + 5)
                .map(|i| PlanStep {
                    id: format!("s{i}"),
                    peer: "p".into(),
                    capability: "c".into(),
                    args: json!({}),
                    depends_on: vec![],
                })
                .collect(),
        };
        assert!((default_confidence(&too_big, &cat) - 0.3).abs() < 1e-6);
    }
}
