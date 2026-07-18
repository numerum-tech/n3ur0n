//! Typed plan + reference resolver + executor for plan-then-execute mode.
//!
//! The LLM "compiles" a structured `Plan` upfront (one LLM call). The
//! executor walks the dependency graph deterministically: reference
//! resolution from a per-dispatch blackboard, ready steps running
//! concurrently up to `MAX_CONCURRENT_STEPS` (see `execute_plan_streaming`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use n3ur0n_core::message::ProtocolVerb;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::Semaphore;

/// Hard cap on concurrent step invocations within a single plan dispatch.
/// Independent steps still run in parallel up to this bound; beyond it they
/// queue. Protects slow upstreams (single-GPU Ollama, rate-limited APIs)
/// from saturating when a plan fans out widely.
const MAX_CONCURRENT_STEPS: usize = 4;

use crate::client as peer_client;
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::catalog::Catalog;
use crate::planner::{DispatchEvent, EventSender, TraceEntry};

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

/// Hard cap on plan size. A small (≤8B) model emitting more than this is
/// almost always hallucinating a fictitious decomposition; better to reject
/// and let the reflect step compose from prior knowledge.
pub const MAX_PLAN_STEPS: usize = 8;

/// Validate the plan against the catalog and structural rules:
/// - non-empty
/// - bounded by `MAX_PLAN_STEPS`
/// - unique step ids
/// - each (peer, capability) resolves in catalog
/// - args validate against the capability's declared `schema_in`. Templates
///   (`${...}`) cause the args to be skipped (they'll be resolved at exec
///   time); we only validate the *literal* arg structure.
/// - declared `depends_on` references existing step ids
/// - acyclic
pub fn validate_plan(plan: &Plan, catalog: &Catalog) -> Result<(), PlanError> {
    if plan.plan.is_empty() {
        return Err(PlanError::Validation("plan has no steps".into()));
    }
    if plan.plan.len() > MAX_PLAN_STEPS {
        return Err(PlanError::Validation(format!(
            "plan has {} steps, exceeds MAX_PLAN_STEPS={}",
            plan.plan.len(),
            MAX_PLAN_STEPS
        )));
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
        let Some(tool) = catalog.find(&tool_name) else {
            return Err(PlanError::Validation(format!(
                "step `{}`: tool `{}` not in catalog",
                step.id, tool_name
            )));
        };

        // args validate against the cap's input schema — but only if the
        // args contain no unresolved templates. Templates are checked + run
        // at execution time when the blackboard is populated.
        if first_unresolved_template(&step.args).is_none()
            && let Err(e) = validate_args_against_schema(&step.args, &tool.cap.schema_in)
        {
            return Err(PlanError::Validation(format!(
                "step `{}`: args do not conform to `{}` schema_in: {}",
                step.id, tool_name, e
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

/// Validate `args` against `schema`. Returns a short error message on
/// failure. If `schema` is empty / non-object (degenerate published cap),
/// we accept anything.
fn validate_args_against_schema(args: &Value, schema: &Value) -> Result<(), String> {
    // Treat `{}` or non-object schemas as "no constraints" — common for
    // pass-through caps and for malformed publishers we shouldn't punish
    // the planner for.
    if !schema.is_object() || schema.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        return Ok(());
    }
    let compiled = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|e| format!("schema compile failed: {e}"))?;
    let result = compiled.validate(args);
    match result {
        Ok(()) => Ok(()),
        Err(errors) => {
            let msgs: Vec<String> = errors.take(3).map(|e| e.to_string()).collect();
            Err(msgs.join("; "))
        }
    }
}

/// Return step ids in a valid execution order (Kahn's algorithm).
///
/// Stable: independent steps preserve their plan declaration order. Steps
/// freed by a parent's completion enter the queue in plan order too. The UI
/// numbers chips left-to-right in plan order, so this keeps execution and
/// display aligned.
pub fn topological_order(plan: &Plan) -> Result<Vec<String>, PlanError> {
    // Plan-index per id for stable enqueue order.
    let plan_idx: HashMap<&str, usize> = plan
        .plan
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    let mut indeg: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in &plan.plan {
        indeg.entry(s.id.clone()).or_insert(0);
        edges.entry(s.id.clone()).or_default();
    }
    for s in &plan.plan {
        let mut deps: HashSet<String> = s.depends_on.iter().cloned().collect();
        collect_refs(&s.args, &mut deps);
        for dep in &deps {
            if dep == &s.id {
                continue;
            }
            if !indeg.contains_key(dep) {
                continue;
            }
            edges.entry(dep.clone()).or_default().push(s.id.clone());
            *indeg.entry(s.id.clone()).or_insert(0) += 1;
        }
    }
    // Sort each adjacency list by plan index so freed steps enqueue in
    // declaration order.
    for outs in edges.values_mut() {
        outs.sort_by_key(|id| plan_idx.get(id.as_str()).copied().unwrap_or(usize::MAX));
        outs.dedup();
    }

    // Seed queue in plan order (FIFO).
    let mut queue: VecDeque<String> = VecDeque::new();
    for s in &plan.plan {
        if indeg.get(&s.id).copied() == Some(0) {
            queue.push_back(s.id.clone());
        }
    }
    let mut order = Vec::new();
    while let Some(id) = queue.pop_front() {
        order.push(id.clone());
        if let Some(outs) = edges.get(&id).cloned() {
            for o in outs {
                if let Some(d) = indeg.get_mut(&o) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push_back(o);
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
        if i + 1 < bytes.len()
            && bytes[i] == b'$'
            && bytes[i + 1] == b'{'
            && let Some(end) = s[i + 2..].find('}')
        {
            let inner = &s[i + 2..i + 2 + end];
            keys.push(inner.to_string());
            i = i + 2 + end + 1;
            continue;
        }
        // Lenient bare `{stepid.path}` — head must look like a step id
        // (alphanum, starts with letter, no whitespace).
        if bytes[i] == b'{'
            && let Some(end) = s[i + 1..].find('}')
        {
            let inner = &s[i + 1..i + 1 + end];
            let head = inner.split('.').next().unwrap_or("");
            if looks_like_step_id(head) {
                keys.push(inner.to_string());
            }
            i = i + 1 + end + 1;
            continue;
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

/// Walk a Value and return the first unresolved `${...}` template found in
/// any string. Used post-substitution to fail fast instead of sending
/// literal `${...}` to a downstream tool.
pub(crate) fn first_unresolved_template(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            // Look for `${...}` substring
            if let Some(start) = s.find("${")
                && let Some(end_rel) = s[start + 2..].find('}')
            {
                return Some(s[start..start + 2 + end_rel + 1].to_string());
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(first_unresolved_template),
        Value::Object(o) => o.values().find_map(first_unresolved_template),
        _ => None,
    }
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
            if let Some(inner) = whole_template(s)
                && let Some(resolved) = lookup_path(&inner, blackboard)
            {
                return resolved;
            }
            // ref unresolved → fall through to inline form (renders as text)
            // Inline substitution.
            Value::String(substitute_inline(s, blackboard))
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|x| resolve_value(x, blackboard)).collect())
        }
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
        if i + 1 < bytes.len()
            && bytes[i] == b'$'
            && bytes[i + 1] == b'{'
            && let Some(end) = s[i + 2..].find('}')
        {
            let inner = &s[i + 2..i + 2 + end];
            let rendered = lookup_path(inner, blackboard)
                .map(value_to_text)
                .unwrap_or_else(|| format!("${{{inner}}}"));
            out.push_str(&rendered);
            i = i + 2 + end + 1;
            continue;
        }
        // Lenient fallback: bare `{stepid.path}` — only when the inner
        // first segment matches a known blackboard key (avoids triggering
        // on legitimate curly-brace literals like "see {readme}").
        if bytes[i] == b'{'
            && let Some(end) = s[i + 1..].find('}')
        {
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
    execute_plan_streaming(node, plan, catalog, None, None).await
}

/// Execute the plan with maximum safe parallelism: every step whose
/// dependencies are satisfied runs concurrently. Independent steps therefore
/// finish in roughly the time of the slowest step instead of summing up.
///
/// Optional `events` channel emits `StepStart` / `StepDone` as work begins
/// and completes (so the UI can light chips up live). Send failures are
/// silently ignored — the executor never blocks on a dropped subscriber.
///
/// The returned `PlanRun.trace` is sorted in plan declaration order so that
/// persisted tool turns + UI history stay deterministic regardless of
/// completion timing.
///
/// `on_step_done` is an optional durability hook invoked synchronously the
/// moment each step finishes, with its plan-declaration index and finalized
/// trace entry. Callers use it to persist the step's tool turns incrementally
/// at a reserved seq (so a crash mid-run leaves a partial, correctly-ordered
/// trace). The index is the step's position in `plan.plan`, not its completion
/// order.
#[allow(clippy::type_complexity)] // the on_step_done callback type is clearer inline than aliased
pub async fn execute_plan_streaming(
    node: &Node,
    plan: &Plan,
    catalog: &Catalog,
    events: Option<&EventSender>,
    mut on_step_done: Option<&mut (dyn FnMut(usize, &TraceEntry) + Send)>,
) -> NodeResult<PlanRun> {
    use futures::FutureExt;
    use futures::stream::{FuturesUnordered, StreamExt};

    // Validate the plan can be ordered (cycle detection); the actual order
    // is irrelevant here — we drive execution off the dependency graph.
    topological_order(plan).map_err(|e| NodeError::InvalidPayload(e.to_string()))?;

    let by_id: HashMap<&str, &PlanStep> = plan.plan.iter().map(|s| (s.id.as_str(), s)).collect();
    let plan_idx: HashMap<String, usize> = plan
        .plan
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();

    // Build dependency edges (declared + ref-inferred) and indegrees.
    let mut indeg: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in &plan.plan {
        indeg.entry(s.id.clone()).or_insert(0);
        edges.entry(s.id.clone()).or_default();
    }
    for s in &plan.plan {
        let mut deps: HashSet<String> = s.depends_on.iter().cloned().collect();
        collect_refs(&s.args, &mut deps);
        for dep in &deps {
            if dep == &s.id || !indeg.contains_key(dep) {
                continue;
            }
            edges.entry(dep.clone()).or_default().push(s.id.clone());
            *indeg.entry(s.id.clone()).or_insert(0) += 1;
        }
    }
    for outs in edges.values_mut() {
        outs.sort_by_key(|id| plan_idx.get(id).copied().unwrap_or(usize::MAX));
        outs.dedup();
    }

    let http = peer_client::http_client();
    let node_for_steps = node.clone();
    let step_sem = Arc::new(Semaphore::new(MAX_CONCURRENT_STEPS));
    let mut blackboard: HashMap<String, Value> = HashMap::new();
    let mut trace_by_id: HashMap<String, TraceEntry> = HashMap::new();
    let mut last_id: Option<String> = None;

    // Steps that have indeg 0 and are queued / in-flight / done.
    let mut ready: VecDeque<String> = plan
        .plan
        .iter()
        .filter(|s| indeg.get(&s.id).copied() == Some(0))
        .map(|s| s.id.clone())
        .collect();

    let mut in_flight: FuturesUnordered<_> = FuturesUnordered::new();

    // Helper: schedule one step (run synchronously up to the await point so
    // resolve_value reads the latest blackboard, then push the future).
    let spawn_step = |step_id: String,
                      blackboard: &HashMap<String, Value>,
                      trace_by_id: &mut HashMap<String, TraceEntry>,
                      in_flight: &mut FuturesUnordered<_>| {
        let step = match by_id.get(step_id.as_str()).copied() {
            Some(s) => s,
            None => return,
        };
        // One stable id per step, shared by the trace entry and the persisted
        // ToolCall/ToolResult pair so DB and in-memory views stay linked.
        let call_id = format!("call_{}", uuid::Uuid::new_v4().simple());
        let resolved_args = resolve_value(&step.args, blackboard);

        // Fail-fast: leftover `${...}` after resolution.
        if let Some(unresolved) = first_unresolved_template(&resolved_args) {
            let err = format!(
                "step `{}`: unresolved template `{}` (no expressions allowed in ${{...}}; \
use raw refs only, let the downstream tool combine values)",
                step.id, unresolved
            );
            trace_by_id.insert(
                step.id.clone(),
                TraceEntry {
                    call_id: call_id.clone(),
                    peer_id: step.peer.clone(),
                    capability: step.capability.clone(),
                    args: resolved_args,
                    result: None,
                    error: Some(err.clone()),
                },
            );
            // Synthesise an instant-complete future so the main loop sees it.
            let id = step.id.clone();
            in_flight.push(
                async move {
                    StepCompletion {
                        id,
                        result: Err(err),
                    }
                }
                .boxed(),
            );
            return;
        }

        let tool_name = format!("{}::{}", step.peer, step.capability);
        let tool = catalog.find(&tool_name).cloned();
        let tool = match tool {
            Some(t) => t,
            None => {
                let err = format!("tool `{tool_name}` not in catalog");
                trace_by_id.insert(
                    step.id.clone(),
                    TraceEntry {
                        call_id: call_id.clone(),
                        peer_id: step.peer.clone(),
                        capability: step.capability.clone(),
                        args: resolved_args,
                        result: None,
                        error: Some(err.clone()),
                    },
                );
                let id = step.id.clone();
                in_flight.push(
                    async move {
                        StepCompletion {
                            id,
                            result: Err(err),
                        }
                    }
                    .boxed(),
                );
                return;
            }
        };

        // Revalidate the *resolved* args against the cap's input schema.
        // `validate_plan` skips templated args at compile time (the values
        // aren't known yet); now that `${...}` refs are substituted we can
        // catch a type error (e.g. an int landing in a string field) locally
        // instead of shipping a doomed, signed invoke over the wire.
        if let Err(e) = validate_args_against_schema(&resolved_args, &tool.cap.schema_in) {
            let err = format!(
                "step `{}`: resolved args do not conform to `{}` schema_in: {}",
                step.id, tool_name, e
            );
            trace_by_id.insert(
                step.id.clone(),
                TraceEntry {
                    call_id: call_id.clone(),
                    peer_id: tool.peer_id.clone(),
                    capability: step.capability.clone(),
                    args: resolved_args,
                    result: None,
                    error: Some(err.clone()),
                },
            );
            let id = step.id.clone();
            in_flight.push(
                async move {
                    StepCompletion {
                        id,
                        result: Err(err),
                    }
                }
                .boxed(),
            );
            return;
        }

        // Stash the trace entry now (peer_id resolved, args captured).
        trace_by_id.insert(
            step.id.clone(),
            TraceEntry {
                call_id: call_id.clone(),
                peer_id: tool.peer_id.clone(),
                capability: step.capability.clone(),
                args: resolved_args.clone(),
                result: None,
                error: None,
            },
        );

        // Build the actual invocation future. It captures clones so it can
        // outlive the borrow on `blackboard`. A semaphore permit (acquired
        // inside the future) caps concurrent in-flight invocations.
        let id = step.id.clone();
        let backend = node_for_steps.backend().clone();
        let keypair = node_for_steps.keypair().clone();
        let http_client = http.clone();
        let our_endpoint = node_for_steps.config().endpoint.clone();
        let cap_name = tool.cap.name.clone();
        let endpoint = tool.peer_endpoint.clone();
        let sem = step_sem.clone();
        let evt_tx = events.cloned();
        let node_exec = node_for_steps.clone();

        in_flight.push(
            async move {
                let _permit = sem
                    .acquire_owned()
                    .await
                    .expect("step semaphore never closed");
                if let Some(tx) = &evt_tx {
                    let _ = tx.send(DispatchEvent::StepStart { id: id.clone() });
                }
                let result: Result<Value, String> = if endpoint.is_none() {
                    backend
                        .invoke(&cap_name, resolved_args.clone())
                        .await
                        .map_err(|e| e.to_string())
                } else {
                    let ep = endpoint.as_deref().unwrap();
                    let invoke_args = match crate::blob_resolve::prepare_invoke_args(
                        &node_exec,
                        &http_client,
                        ep,
                        &cap_name,
                        resolved_args.clone(),
                    )
                    .await
                    {
                        Ok(a) => a,
                        Err(e) => return StepCompletion { id, result: Err(e) },
                    };
                    let payload = json!({
                        "capability": cap_name,
                        "args": invoke_args,
                    });
                    match peer_client::send_signed(
                        &http_client,
                        &keypair,
                        ep,
                        ProtocolVerb::Invoke,
                        payload,
                        our_endpoint.as_deref(),
                    )
                    .await
                    {
                        Ok(reply) => {
                            let p = reply.envelope.payload;
                            let raw = p.get("result").cloned().unwrap_or(p);
                            crate::blob_resolve::fetch_output_blobs(
                                &node_exec,
                                &http_client,
                                ep,
                                raw,
                            )
                            .await
                        }
                        Err(e) => Err(e.to_string()),
                    }
                };
                StepCompletion { id, result }
            }
            .boxed(),
        );
    };

    // Initial seed.
    while let Some(step_id) = ready.pop_front() {
        spawn_step(step_id, &blackboard, &mut trace_by_id, &mut in_flight);
    }

    // Drain completions, free dependents, schedule them.
    while let Some(completion) = in_flight.next().await {
        let StepCompletion { id, result } = completion;

        match result {
            Ok(value) => {
                blackboard.insert(id.clone(), value.clone());
                if let Some(entry) = trace_by_id.get_mut(&id) {
                    entry.result = Some(value.clone());
                }
                if let Some(tx) = events {
                    let _ = tx.send(DispatchEvent::StepDone {
                        id: id.clone(),
                        result: Some(value),
                        error: None,
                    });
                }
                last_id = Some(id.clone());
            }
            Err(err) => {
                blackboard.insert(id.clone(), json!({"error": err.clone()}));
                if let Some(entry) = trace_by_id.get_mut(&id) {
                    entry.error = Some(err.clone());
                }
                if let Some(tx) = events {
                    let _ = tx.send(DispatchEvent::StepDone {
                        id: id.clone(),
                        result: None,
                        error: Some(err),
                    });
                }
            }
        }

        // Durability hook: persist this step's tool pair the moment it
        // completes, at a seq reserved by its plan index. Steps finish out of
        // order under concurrency, but the reserved seq keeps the persisted
        // trace in plan declaration order.
        if let Some(cb) = on_step_done.as_deref_mut()
            && let (Some(idx), Some(entry)) = (plan_idx.get(&id).copied(), trace_by_id.get(&id))
        {
            cb(idx, entry);
        }

        // Free downstream steps.
        if let Some(outs) = edges.get(&id).cloned() {
            for o in outs {
                if let Some(d) = indeg.get_mut(&o) {
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        spawn_step(o, &blackboard, &mut trace_by_id, &mut in_flight);
                    }
                }
            }
        }
    }

    // Reassemble trace in plan declaration order for stable persistence.
    let mut trace: Vec<TraceEntry> = Vec::with_capacity(trace_by_id.len());
    for s in &plan.plan {
        if let Some(entry) = trace_by_id.remove(&s.id) {
            trace.push(entry);
        }
    }

    Ok(PlanRun {
        blackboard,
        last_step_id: last_id,
        trace,
    })
}

struct StepCompletion {
    id: String,
    result: Result<Value, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
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
    fn detects_unresolved_template() {
        let bb = bb(&[("s1", json!({"value": 42}))]);
        let resolved = resolve_value(&json!("year ${s1.value + s2.year}"), &bb);
        // The expression form doesn't match any path key, falls through to
        // inline substitute, which leaves it literal because the inner is
        // not a clean dotted path.
        let leftover = first_unresolved_template(&resolved);
        assert!(leftover.is_some(), "unresolved template should be flagged");
    }

    #[test]
    fn topo_order_independent_steps_preserve_plan_order() {
        // 12 independent steps — no deps. Must come out in declaration order.
        let plan = Plan {
            plan: (1..=12)
                .map(|i| PlanStep {
                    id: format!("s{i}"),
                    peer: "p".into(),
                    capability: "c".into(),
                    args: json!({}),
                    depends_on: vec![],
                })
                .collect(),
        };
        let order = topological_order(&plan).unwrap();
        let expected: Vec<String> = (1..=12).map(|i| format!("s{i}")).collect();
        assert_eq!(order, expected);
    }

    fn make_catalog(name: &str, peer: &str, schema_in: Value) -> Catalog {
        use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
        let mut cat = Catalog::default();
        cat.tools.push(crate::planner::catalog::ToolDef {
            peer_id: format!("n3:{peer}aaaaaaaaaaaa"),
            peer_endpoint: Some(format!("http://{peer}:4242")),
            cap: CapabilityDecl {
                name: name.into(),
                description: format!("test {name}"),
                schema_in,
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
                version: "0.0.0".into(),
                languages: vec![],
                countries: vec![],
            },
        });
        cat
    }

    #[test]
    fn validate_plan_rejects_more_than_max_steps() {
        let cat = make_catalog("c", "peera", json!({}));
        let plan = Plan {
            plan: (1..=MAX_PLAN_STEPS + 1)
                .map(|i| PlanStep {
                    id: format!("s{i}"),
                    peer: short_peer_helper("peera"),
                    capability: "c".into(),
                    args: json!({}),
                    depends_on: vec![],
                })
                .collect(),
        };
        let err = validate_plan(&plan, &cat).unwrap_err().to_string();
        assert!(err.contains("MAX_PLAN_STEPS"), "got: {err}");
    }

    #[test]
    fn validate_plan_rejects_args_violating_schema_in() {
        let schema = json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        });
        let cat = make_catalog("reverse", "peera", schema);
        let plan = Plan {
            plan: vec![PlanStep {
                id: "s1".into(),
                peer: short_peer_helper("peera"),
                capability: "reverse".into(),
                // Missing required `text` — must reject.
                args: json!({"wrong_field": 42}),
                depends_on: vec![],
            }],
        };
        let err = validate_plan(&plan, &cat).unwrap_err().to_string();
        assert!(err.contains("schema_in"), "got: {err}");
    }

    #[test]
    fn validate_plan_accepts_args_with_unresolved_template() {
        // Templates skip schema validation — they are resolved at exec time.
        let schema = json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        });
        let cat = make_catalog("reverse", "peera", schema);
        let plan = Plan {
            plan: vec![PlanStep {
                id: "s1".into(),
                peer: short_peer_helper("peera"),
                capability: "reverse".into(),
                args: json!({"text": "${s0.value}"}),
                depends_on: vec![],
            }],
        };
        assert!(validate_plan(&plan, &cat).is_ok());
    }

    fn short_peer_helper(peer: &str) -> String {
        // Mirror catalog's short_peer behaviour: drop "n3:" prefix, take 12.
        let full = format!("n3:{peer}aaaaaaaaaaaa");
        let trimmed = full.strip_prefix("n3:").unwrap_or(&full);
        trimmed.chars().take(12).collect()
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

    // --- P3: revalidation of resolved args against schema_in ---

    /// Local backend that echoes args back and counts how many times it was
    /// invoked, so a test can assert that a step which fails post-resolution
    /// validation never reaches the backend.
    #[derive(Debug)]
    struct CountingEchoBackend {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl n3ur0n_adapters::Backend for CountingEchoBackend {
        async fn invoke(
            &self,
            _capability: &str,
            args: Value,
        ) -> n3ur0n_adapters::AdapterResult<Value> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(args)
        }

        async fn describe(
            &self,
        ) -> n3ur0n_adapters::AdapterResult<Vec<n3ur0n_core::CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> n3ur0n_adapters::AdapterResult<n3ur0n_adapters::HealthStatus> {
            Ok(n3ur0n_adapters::HealthStatus::Healthy)
        }
    }

    /// Two local caps under one peer: `gen` (no schema) and `consume`
    /// (requires `text: string`). Both `peer_endpoint: None` → executed via
    /// the node's local backend, never over the network.
    fn make_local_catalog() -> Catalog {
        use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
        let decl = |name: &str, schema_in: Value| CapabilityDecl {
            name: name.into(),
            description: format!("test {name}"),
            schema_in,
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
            version: "0.0.0".into(),
            languages: vec![],
            countries: vec![],
        };
        let mut cat = Catalog::default();
        let peer_id = "n3:peeraaaaaaaaaaaa".to_string();
        cat.tools.push(crate::planner::catalog::ToolDef {
            peer_id: peer_id.clone(),
            peer_endpoint: None,
            cap: decl("gen", json!({})),
        });
        cat.tools.push(crate::planner::catalog::ToolDef {
            peer_id,
            peer_endpoint: None,
            cap: decl(
                "consume",
                json!({
                    "type": "object",
                    "required": ["text"],
                    "properties": {"text": {"type": "string"}}
                }),
            ),
        });
        cat
    }

    #[tokio::test]
    async fn resolved_args_violating_schema_fail_locally_without_invoke() {
        use n3ur0n_adapters::Backend;
        use n3ur0n_core::Keypair;
        use n3ur0n_storage::open_in_memory;
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend: Arc<dyn Backend> = Arc::new(CountingEchoBackend {
            calls: calls.clone(),
        });
        let db = open_in_memory().unwrap();
        let registry = crate::registry::CapabilityRegistry::from_decls(vec![]);
        let node = crate::node::Node::new(
            Keypair::generate(),
            db,
            backend,
            registry,
            crate::node::NodeConfig::default(),
        );

        let cat = make_local_catalog();
        let peer = short_peer_helper("peera");
        let plan = Plan {
            plan: vec![
                // s1 echoes `{value: 42}` → blackboard["s1"] = {value: 42}.
                PlanStep {
                    id: "s1".into(),
                    peer: peer.clone(),
                    capability: "gen".into(),
                    args: json!({"value": 42}),
                    depends_on: vec![],
                },
                // s2 resolves `${s1.value}` (int 42) into a string field.
                PlanStep {
                    id: "s2".into(),
                    peer,
                    capability: "consume".into(),
                    args: json!({"text": "${s1.value}"}),
                    depends_on: vec![],
                },
            ],
        };

        let run = execute_plan(&node, &plan, &cat).await.unwrap();

        // Only s1 reached the backend; s2 failed schema revalidation locally.
        assert_eq!(calls.load(Ordering::SeqCst), 1, "s2 must not be invoked");

        let s2 = run
            .trace
            .iter()
            .find(|e| e.capability == "consume")
            .expect("s2 trace present");
        let err = s2.error.as_deref().unwrap_or("");
        assert!(
            err.contains("schema_in"),
            "expected schema error, got: {err:?}"
        );
    }

    // --- P1: incremental tool-turn persistence (durability hook) ---

    /// Local backend: `gen` echoes args; `block` never returns. Lets a test
    /// stall execution at a chosen step to simulate a crash mid-run.
    #[derive(Debug)]
    struct BlockingBackend;

    #[async_trait::async_trait]
    impl n3ur0n_adapters::Backend for BlockingBackend {
        async fn invoke(
            &self,
            capability: &str,
            args: Value,
        ) -> n3ur0n_adapters::AdapterResult<Value> {
            if capability == "block" {
                futures::future::pending::<()>().await;
            }
            Ok(args)
        }

        async fn describe(
            &self,
        ) -> n3ur0n_adapters::AdapterResult<Vec<n3ur0n_core::CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> n3ur0n_adapters::AdapterResult<n3ur0n_adapters::HealthStatus> {
            Ok(n3ur0n_adapters::HealthStatus::Healthy)
        }
    }

    fn local_cap_catalog(names: &[&str]) -> Catalog {
        use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
        let mut cat = Catalog::default();
        for name in names {
            cat.tools.push(crate::planner::catalog::ToolDef {
                peer_id: "n3:peeraaaaaaaaaaaa".into(),
                peer_endpoint: None,
                cap: CapabilityDecl {
                    name: (*name).into(),
                    description: format!("test {name}"),
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
                    version: "0.0.0".into(),
                    languages: vec![],
                    countries: vec![],
                },
            });
        }
        cat
    }

    #[tokio::test]
    async fn step_turns_persisted_incrementally_survive_a_kill() {
        use crate::conversation::persist_tool_pair_at;
        use n3ur0n_adapters::Backend;
        use n3ur0n_core::Keypair;
        use n3ur0n_storage::conversations::{self, ConversationRecord};
        use n3ur0n_storage::open_in_memory;
        use std::sync::Arc;

        let backend: Arc<dyn Backend> = Arc::new(BlockingBackend);
        let db = open_in_memory().unwrap();
        // Seed a conversation row so the FK + append_turn UPDATE resolve.
        conversations::insert(
            &db,
            &ConversationRecord {
                id: "conv1".into(),
                client_id: "client".into(),
                title: None,
                created_at: 0,
                updated_at: 0,
            },
        )
        .unwrap();
        let registry = crate::registry::CapabilityRegistry::from_decls(vec![]);
        let node = crate::node::Node::new(
            Keypair::generate(),
            db.clone(),
            backend,
            registry,
            crate::node::NodeConfig::default(),
        );

        // Chain so steps run one at a time: s1 → s2 → s3(block) → s4.
        let peer = short_peer_helper("peera");
        let cat = local_cap_catalog(&["gen", "block"]);
        let step = |id: &str, cap: &str, dep: Option<&str>| PlanStep {
            id: id.into(),
            peer: peer.clone(),
            capability: cap.into(),
            args: json!({"v": id}),
            depends_on: dep.into_iter().map(|d| d.to_string()).collect(),
        };
        let plan = Plan {
            plan: vec![
                step("s1", "gen", None),
                step("s2", "gen", Some("s1")),
                step("s3", "block", Some("s2")),
                step("s4", "gen", Some("s3")),
            ],
        };

        // Durability hook mirroring dispatch_inner: base_seq = user turn seq.
        // No user turn here, so base_seq = 0 → first tool turn lands at seq 1.
        let base_seq = 0i64;
        let db_hook = db.clone();
        let fallback = node.instance_id();
        let mut on_done = |idx: usize, entry: &TraceEntry| {
            let pid =
                n3ur0n_core::InstanceId::parse(&entry.peer_id).unwrap_or_else(|_| fallback.clone());
            persist_tool_pair_at(
                &db_hook,
                "conv1",
                base_seq,
                idx,
                &entry.call_id,
                &pid,
                &entry.capability,
                &entry.args,
                &entry.result,
                &entry.error,
                0,
            )
            .unwrap();
        };

        // Kill the run mid-execution: s3 blocks forever, so the timeout drops
        // the future after s1 and s2 have completed + persisted.
        let fut = execute_plan_streaming(&node, &plan, &cat, None, Some(&mut on_done));
        let killed = tokio::time::timeout(std::time::Duration::from_millis(200), fut).await;
        assert!(killed.is_err(), "run should have been killed by timeout");

        // Exactly 2 tool pairs (s1, s2) persisted, at the reserved seqs.
        let turns = conversations::load_turns(&db, "conv1").unwrap();
        let seqs: Vec<i64> = turns.iter().map(|t| t.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2, 3, 4],
            "two pairs at reserved seqs, got {seqs:?}"
        );
        let roles: Vec<&str> = turns.iter().map(|t| t.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["tool_call", "tool_result", "tool_call", "tool_result"]
        );
    }
}
