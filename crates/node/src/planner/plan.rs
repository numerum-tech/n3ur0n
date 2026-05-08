//! Typed plan + reference resolver + executor for plan-then-execute mode.
//!
//! The LLM "compiles" a structured `Plan` upfront (one LLM call). The
//! executor walks it deterministically: topological order over `depends_on`,
//! reference resolution from a per-conversation blackboard, sequential
//! execution v0.1 (parallelism for independent steps = v0.2).

use std::collections::{HashMap, HashSet};

use n3ur0n_core::message::ProtocolVerb;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::client as peer_client;
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::catalog::{Catalog, ToolDef};
use crate::planner::TraceEntry;

/// One step in a plan.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanStep {
    /// Unique within the plan (e.g. "s1").
    pub id: String,
    /// Short peer id (matches the form used in `Catalog::tool_name`).
    pub peer: String,
    pub capability: String,
    /// Free-form args. May contain references like `${s1.value}` or
    /// `"prompt with ${s1.field}"`.
    #[serde(default)]
    pub args: Value,
    /// Step ids that must complete before this one. Optional; we infer
    /// missing dependencies from references too.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// LLM-emitted plan structure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Plan {
    pub plan: Vec<PlanStep>,
}

/// Result of executing a plan.
#[derive(Debug, Clone)]
pub struct PlanRun {
    /// `step_id -> result` map. Errors stored as `{"error": "..."}`.
    pub blackboard: HashMap<String, Value>,
    /// Last step id (typically what the user wants summarised).
    pub last_step_id: Option<String>,
    /// Per-step trace entries for the UI panel.
    pub trace: Vec<TraceEntry>,
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("plan parse error: {0}")]
    Parse(String),
    #[error("plan validation: {0}")]
    Validation(String),
    #[error("plan execution: {0}")]
    Execution(String),
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate the plan against the catalog and structural rules:
/// - non-empty
/// - unique step ids
/// - each (peer, capability) resolves in catalog
/// - declared `depends_on` references existing step ids
/// - acyclic
pub fn validate_plan(plan: &Plan, catalog: &Catalog) -> Result<(), PlanError> {
    if plan.plan.is_empty() {
        return Err(PlanError::Validation("plan has no steps".into()));
    }
    let mut seen_ids: HashSet<&str> = HashSet::new();
    for step in &plan.plan {
        if !seen_ids.insert(step.id.as_str()) {
            return Err(PlanError::Validation(format!(
                "duplicate step id `{}`",
                step.id
            )));
        }
    }
    for step in &plan.plan {
        // tool resolves
        let tool_name = format!("{}::{}", step.peer, step.capability);
        if catalog.find(&tool_name).is_none() {
            return Err(PlanError::Validation(format!(
                "step `{}`: tool `{}` not in catalog",
                step.id, tool_name
            )));
        }
        // depends_on refs known
        for dep in &step.depends_on {
            if !plan.plan.iter().any(|s| &s.id == dep) {
                return Err(PlanError::Validation(format!(
                    "step `{}`: depends_on `{}` does not exist",
                    step.id, dep
                )));
            }
        }
    }
    // Cycle detection via Kahn-style topological sort over declared edges
    // PLUS edges inferred from `${id...}` references in args.
    let order = topological_order(plan)?;
    if order.len() != plan.plan.len() {
        return Err(PlanError::Validation("plan has a dependency cycle".into()));
    }
    Ok(())
}

/// Return step ids in a valid execution order (Kahn's algorithm).
pub fn topological_order(plan: &Plan) -> Result<Vec<String>, PlanError> {
    // Build edges: declared depends_on + references inferred from args.
    let mut indeg: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in &plan.plan {
        indeg.entry(s.id.clone()).or_insert(0);
        edges.entry(s.id.clone()).or_default();
    }
    for s in &plan.plan {
        let mut deps: HashSet<String> = s.depends_on.iter().cloned().collect();
        // Walk args, look for ${id...} patterns and add as deps.
        collect_refs(&s.args, &mut deps);
        for dep in &deps {
            if dep == &s.id {
                continue;
            }
            if !indeg.contains_key(dep) {
                continue; // unknown id, validation will catch elsewhere
            }
            edges.entry(dep.clone()).or_default().push(s.id.clone());
            *indeg.entry(s.id.clone()).or_insert(0) += 1;
        }
    }
    let mut queue: Vec<String> = indeg
        .iter()
        .filter(|&(_, d)| *d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut order = Vec::new();
    while let Some(id) = queue.pop() {
        order.push(id.clone());
        if let Some(outs) = edges.get(&id).cloned() {
            for o in outs {
                if let Some(d) = indeg.get_mut(&o) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push(o);
                    }
                }
            }
        }
    }
    Ok(order)
}

/// Collect referenced step ids from any `${id...}` template in the value.
pub fn collect_refs(v: &Value, out: &mut HashSet<String>) {
    match v {
        Value::String(s) => {
            for r in extract_template_keys(s) {
                let id = r.split('.').next().unwrap_or("");
                if !id.is_empty() {
                    out.insert(id.to_string());
                }
            }
        }
        Value::Array(arr) => arr.iter().for_each(|x| collect_refs(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

fn extract_template_keys(s: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Preferred `${...}`
        if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let inner = &s[i + 2..i + 2 + end];
                keys.push(inner.to_string());
                i = i + 2 + end + 1;
                continue;
            }
        }
        // Lenient bare `{stepid.path}` — head must look like a step id
        // (alphanum, starts with letter, no whitespace).
        if bytes[i] == b'{' {
            if let Some(end) = s[i + 1..].find('}') {
                let inner = &s[i + 1..i + 1 + end];
                let head = inner.split('.').next().unwrap_or("");
                if looks_like_step_id(head) {
                    keys.push(inner.to_string());
                }
                i = i + 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    keys
}

fn looks_like_step_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Reference resolution
// ---------------------------------------------------------------------------

/// Resolve `${id.path.to.value}` references inside `v` against the
/// blackboard. Returns a fresh Value with substitutions applied.
///
/// Rules:
/// - If a Value::String is *exactly* `${...}` → replaced with the resolved
///   Value (may be number, object, etc.).
/// - Otherwise, inline `${...}` occurrences inside a string are replaced
///   with the resolved value rendered as JSON-ish text.
pub fn resolve_value(v: &Value, blackboard: &HashMap<String, Value>) -> Value {
    match v {
        Value::String(s) => {
            // Whole-string single ref?
            if let Some(inner) = whole_template(s) {
                if let Some(resolved) = lookup_path(&inner, blackboard) {
                    return resolved;
                }
                // ref unresolved → fall through to inline form (renders as text)
            }
            // Inline substitution.
            Value::String(substitute_inline(s, blackboard))
        }
        Value::Array(arr) => Value::Array(arr.iter().map(|x| resolve_value(x, blackboard)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), resolve_value(x, blackboard)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn whole_template(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.starts_with("${") && trimmed.ends_with('}') {
        let inner = &trimmed[2..trimmed.len() - 1];
        if !inner.contains("${") && !inner.contains('{') {
            return Some(inner.to_string());
        }
    }
    // Also accept bare `{stepid.path}` when it is the entire trimmed
    // string. The caller validates the head against the blackboard.
    if trimmed.starts_with('{') && trimmed.ends_with('}') && !trimmed.starts_with("${") {
        let inner = &trimmed[1..trimmed.len() - 1];
        if !inner.contains('{') && !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    None
}

fn lookup_path(path: &str, blackboard: &HashMap<String, Value>) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let id = parts.first()?;
    let mut current = blackboard.get(*id)?.clone();
    for p in &parts[1..] {
        current = match current {
            Value::Object(o) => o.get(*p)?.clone(),
            _ => return None,
        };
    }
    Some(current)
}

fn substitute_inline(s: &str, blackboard: &HashMap<String, Value>) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Preferred form: `${stepid.path}`
        if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let inner = &s[i + 2..i + 2 + end];
                let rendered = lookup_path(inner, blackboard)
                    .map(value_to_text)
                    .unwrap_or_else(|| format!("${{{inner}}}"));
                out.push_str(&rendered);
                i = i + 2 + end + 1;
                continue;
            }
        }
        // Lenient fallback: bare `{stepid.path}` — only when the inner
        // first segment matches a known blackboard key (avoids triggering
        // on legitimate curly-brace literals like "see {readme}").
        if bytes[i] == b'{' {
            if let Some(end) = s[i + 1..].find('}') {
                let inner = &s[i + 1..i + 1 + end];
                let head = inner.split('.').next().unwrap_or("");
                if !head.is_empty() && blackboard.contains_key(head) {
                    let rendered = lookup_path(inner, blackboard)
                        .map(value_to_text)
                        .unwrap_or_else(|| format!("{{{inner}}}"));
                    out.push_str(&rendered);
                    i = i + 1 + end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn value_to_text(v: Value) -> String {
    match v {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute the plan sequentially in topological order.
pub async fn execute_plan(node: &Node, plan: &Plan, catalog: &Catalog) -> NodeResult<PlanRun> {
    let order = topological_order(plan).map_err(|e| NodeError::InvalidPayload(e.to_string()))?;
    let by_id: HashMap<&str, &PlanStep> = plan.plan.iter().map(|s| (s.id.as_str(), s)).collect();

    let http = peer_client::http_client();
    let mut blackboard: HashMap<String, Value> = HashMap::new();
    let mut trace: Vec<TraceEntry> = Vec::new();
    let mut last_id: Option<String> = None;

    for step_id in order {
        let step = by_id.get(step_id.as_str()).copied().ok_or_else(|| {
            NodeError::InvalidPayload(format!("topo order yielded unknown id `{step_id}`"))
        })?;
        let resolved_args = resolve_value(&step.args, &blackboard);

        // Find the tool in catalog
        let tool_name = format!("{}::{}", step.peer, step.capability);
        let tool: ToolDef = match catalog.find(&tool_name) {
            Some(t) => t.clone(),
            None => {
                let err = format!("tool `{tool_name}` not in catalog");
                blackboard.insert(step.id.clone(), json!({"error": err}));
                trace.push(TraceEntry {
                    peer_id: step.peer.clone(),
                    capability: step.capability.clone(),
                    args: resolved_args,
                    result: None,
                    error: Some(err),
                });
                continue;
            }
        };

        // Execute: local or remote.
        let outcome: Result<Value, String> = if tool.peer_endpoint.is_none() {
            node.backend()
                .invoke(&tool.cap.name, resolved_args.clone())
                .await
                .map_err(|e| e.to_string())
        } else {
            let endpoint = tool.peer_endpoint.clone().unwrap();
            let payload = json!({
                "capability": tool.cap.name,
                "args": resolved_args,
            });
            match peer_client::send_signed(
                &http,
                node.keypair(),
                &endpoint,
                ProtocolVerb::Invoke,
                payload,
            )
            .await
            {
                Ok(reply) => {
                    let payload = reply.envelope.payload;
                    Ok(payload.get("result").cloned().unwrap_or(payload))
                }
                Err(e) => Err(e.to_string()),
            }
        };

        match outcome {
            Ok(result) => {
                blackboard.insert(step.id.clone(), result.clone());
                trace.push(TraceEntry {
                    peer_id: tool.peer_id.clone(),
                    capability: step.capability.clone(),
                    args: resolved_args,
                    result: Some(result),
                    error: None,
                });
                last_id = Some(step.id.clone());
            }
            Err(err) => {
                blackboard.insert(step.id.clone(), json!({"error": err}));
                trace.push(TraceEntry {
                    peer_id: tool.peer_id.clone(),
                    capability: step.capability.clone(),
                    args: resolved_args,
                    result: None,
                    error: Some(err),
                });
                // Continue executing remaining steps — downstream may still
                // produce a useful partial result. Tools that depend on a
                // failed step receive the `{"error": ...}` blob via refs.
            }
        }
    }

    Ok(PlanRun {
        blackboard,
        last_step_id: last_id,
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn whole_string_ref_returns_value() {
        let blackboard = bb(&[("s1", json!({"value": 42}))]);
        let resolved = resolve_value(&json!("${s1.value}"), &blackboard);
        assert_eq!(resolved, json!(42));
    }

    #[test]
    fn inline_substitution_in_string() {
        let blackboard = bb(&[("s1", json!({"value": 42}))]);
        let resolved = resolve_value(&json!("the answer is ${s1.value}"), &blackboard);
        assert_eq!(resolved, json!("the answer is 42"));
    }

    #[test]
    fn nested_resolution() {
        let blackboard = bb(&[("s1", json!({"reversed": "olleh"}))]);
        let args = json!({
            "messages": [
                {"role": "user", "content": "use ${s1.reversed} please"}
            ]
        });
        let resolved = resolve_value(&args, &blackboard);
        assert_eq!(
            resolved["messages"][0]["content"],
            json!("use olleh please")
        );
    }

    #[test]
    fn missing_ref_kept_literal() {
        let blackboard = bb(&[]);
        let resolved = resolve_value(&json!("${unknown}"), &blackboard);
        assert_eq!(resolved, json!("${unknown}"));
    }

    #[test]
    fn topo_order_simple_chain() {
        let plan = Plan {
            plan: vec![
                PlanStep {
                    id: "s1".into(),
                    peer: "p".into(),
                    capability: "c1".into(),
                    args: json!({}),
                    depends_on: vec![],
                },
                PlanStep {
                    id: "s2".into(),
                    peer: "p".into(),
                    capability: "c2".into(),
                    args: json!({"x": "${s1.value}"}),
                    depends_on: vec![],
                },
            ],
        };
        let order = topological_order(&plan).unwrap();
        assert_eq!(order, vec!["s1".to_string(), "s2".to_string()]);
    }

    #[test]
    fn topo_order_cycle_detected() {
        let plan = Plan {
            plan: vec![
                PlanStep {
                    id: "s1".into(),
                    peer: "p".into(),
                    capability: "c1".into(),
                    args: json!({"x": "${s2.value}"}),
                    depends_on: vec![],
                },
                PlanStep {
                    id: "s2".into(),
                    peer: "p".into(),
                    capability: "c2".into(),
                    args: json!({"x": "${s1.value}"}),
                    depends_on: vec![],
                },
            ],
        };
        let order = topological_order(&plan).unwrap();
        assert!(order.len() < 2);
    }
}
