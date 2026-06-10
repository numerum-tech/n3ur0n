//! Plan-then-execute planner.
//!
//! Two LLM calls per dispatch:
//! 1. **Compile**: emit a typed `Plan` (JSON) referring to peers + caps in
//!    the catalog. Forced JSON output via Ollama `format: "json"`.
//! 2. **Reflect**: compose the user-facing reply from the plan trace +
//!    final blackboard.
//!
//! Between the two, a deterministic executor walks the plan in
//! topological order and substitutes `${step_id.path}` references.

use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::Backend;
use serde_json::{Value, json};
use tracing::warn;

use crate::conversation::{persist_last, ConversationState};
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::catalog::{Catalog, ToolDef};
use crate::planner::plan::{execute_plan_streaming, validate_plan, Plan};
use crate::planner::{
    DispatchEvent, DispatchMode, DispatchOptions, DispatchOutcome, EventSender, Planner,
    PlanStepInfo, TraceEntry, MAX_CONTEXT_TURNS,
};
/// Maximum number of *remote* tools surfaced in the compile prompt. Local
/// tools always pass through (the operator configured them explicitly).
/// 20 picked to keep prompts under ~3k tokens for moderately enriched
/// caps; tune if observed compile latency starts to dominate.
const REMOTE_TOP_K: usize = 20;

#[derive(Clone)]
pub struct PlanExecPlanner {
    /// The compile step is delegated to a `PlanCompiler`. The simple
    /// constructor builds a `LocalLLMCompiler` wrapping `llm_backend`;
    /// callers wanting a cascading or remote compiler use
    /// `PlanExecPlanner::with_compiler`.
    pub compiler: Arc<dyn crate::planner::compiler::PlanCompiler>,
    /// Backend used for the reflect step (final user-facing reply). May
    /// be a different model than the compile-time one; in practice it's
    /// the same backend as the local compiler today.
    pub llm_backend: Arc<dyn Backend>,
    pub model_hint: Option<String>,
}

impl std::fmt::Debug for PlanExecPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanExecPlanner")
            .field("compiler", &self.compiler)
            .field("model_hint", &self.model_hint)
            .finish()
    }
}

impl PlanExecPlanner {
    /// Build the default planner: compile via a `LocalLLMCompiler` over
    /// `llm_backend`, reflect via the same backend.
    pub fn new(llm_backend: Arc<dyn Backend>, model_hint: Option<String>) -> Self {
        let compiler = Arc::new(crate::planner::compiler::LocalLLMCompiler {
            llm_backend: llm_backend.clone(),
            model_hint: model_hint.clone(),
            system_prompt: Arc::new(default_compile_system_prompt),
        });
        Self {
            compiler,
            llm_backend,
            model_hint,
        }
    }

    /// Build a planner with a custom compiler (e.g. `CascadingCompiler`
    /// wrapping a local and a remote compiler). Reflect still uses the
    /// supplied backend.
    pub fn with_compiler(
        compiler: Arc<dyn crate::planner::compiler::PlanCompiler>,
        llm_backend: Arc<dyn Backend>,
        model_hint: Option<String>,
    ) -> Self {
        Self {
            compiler,
            llm_backend,
            model_hint,
        }
    }


    fn reflect_system_prompt(&self) -> String {
        String::from(
            "You are an n3ur0n response composer. The user asked a question and a plan \
was executed; you now have the executor's blackboard (all step results). Write the \
final reply for the user in plain text. Use the user's language. Do NOT emit JSON, \
do NOT call any tool, do NOT re-explain the plan unless the user asked for it. Be \
concise and use the actual values from the blackboard.\n\
\n\
Honesty rules — non-negotiable:\n\
- If the user asked for a side-effect action (sending email, posting, \
payments, modifying files, calling an external service) and no tool capable \
of performing it ran successfully, say plainly that you cannot perform it. \
Do NOT pretend the action was done.\n\
- If a tool step failed but the user gave the data directly (a literal \
string in the prompt, a number, etc.), compute the answer yourself from \
that data when you can. Tool failures on simple known data don't excuse you \
from answering.\n\
- Acknowledge step errors when they prevented you from gathering data the \
user genuinely needed.",
        )
    }
}

#[async_trait]
impl Planner for PlanExecPlanner {
    async fn dispatch(
        &self,
        node: &Node,
        state: &mut ConversationState,
        input: crate::conversation::UserInput,
        _mode: DispatchMode,
        _opts: DispatchOptions,
    ) -> NodeResult<DispatchOutcome> {
        self.dispatch_inner(node, state, input, None).await
    }

    async fn dispatch_streaming(
        &self,
        node: &Node,
        state: &mut ConversationState,
        input: crate::conversation::UserInput,
        _mode: DispatchMode,
        _opts: DispatchOptions,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        self.dispatch_inner(node, state, input, Some(&events))
            .await
    }
}

impl PlanExecPlanner {
    async fn dispatch_inner(
        &self,
        node: &Node,
        state: &mut ConversationState,
        input: crate::conversation::UserInput,
        events: Option<&EventSender>,
    ) -> NodeResult<DispatchOutcome> {
        let planner_text = input.planner_text();
        // 1. Persist user turn.
        state.push_user_input(&input);
        persist_last(node.db(), state)
            .map_err(|e| NodeError::InvalidPayload(format!("persist user: {e}")))?;

        // 2. Build catalog — query-aware: local caps always kept, remote
        // caps ranked against the user message via BM25 and trimmed to the
        // top REMOTE_TOP_K. Keeps prompt size bounded as the network grows.
        let registry_snapshot = node.registry();
        let catalog = Catalog::build_for_query(
            node.instance_id().as_str(),
            &registry_snapshot,
            node.db(),
            500,
            &planner_text,
            REMOTE_TOP_K,
        )?;

        // 3. Compile: delegate to the configured PlanCompiler. The
        // default LocalLLMCompiler ships the constrained-decoding fields
        // (grammar / response_format / format) so backends that honour
        // them stay strict; cascading variants may try a remote planner.
        let plan = self.compiler.compile(&planner_text, &catalog).await?;

        // Surface low-confidence plans to the UI. Threshold matches the
        // default cascade escalation point (0.5) so the chip-row banner
        // appears precisely when a cascade *would* have triggered an
        // escalation — useful even when no remote fallback is configured.
        let confidence = self.compiler.confidence(&plan, &catalog).await;
        if confidence < 0.5 {
            if let Some(tx) = events {
                let _ = tx.send(DispatchEvent::LowConfidence { confidence });
            }
        }

        // 4. Validate.
        if let Err(e) = validate_plan(&plan, &catalog) {
            warn!(error = %e, "plan validation failed; falling back to direct reply");
            if let Some(tx) = events {
                let _ = tx.send(DispatchEvent::PlanReady { steps: Vec::new() });
            }
            return self
                .reflect_only(node, state, &planner_text, None, Vec::new(), events)
                .await;
        }

        // Special case: empty plan = answer directly without any tool.
        if plan.plan.is_empty() {
            if let Some(tx) = events {
                let _ = tx.send(DispatchEvent::PlanReady { steps: Vec::new() });
            }
            return self
                .reflect_only(node, state, &planner_text, None, Vec::new(), events)
                .await;
        }

        // Announce the plan upfront so the UI can render the chip row.
        if let Some(tx) = events {
            let steps: Vec<PlanStepInfo> = plan
                .plan
                .iter()
                .map(|s| {
                    let tool_name = format!("{}::{}", s.peer, s.capability);
                    let tool = catalog.find(&tool_name);
                    let peer_id = tool
                        .map(|t| t.peer_id.clone())
                        .unwrap_or_else(|| s.peer.clone());
                    PlanStepInfo {
                        id: s.id.clone(),
                        peer_id,
                        peer_short: s.peer.clone(),
                        capability: s.capability.clone(),
                    }
                })
                .collect();
            let _ = tx.send(DispatchEvent::PlanReady { steps });
        }

        // 5. Execute.
        let run = execute_plan_streaming(node, &plan, &catalog, events).await?;

        // Persist tool turns for the UI / DB record (sequentially).
        for entry in &run.trace {
            // We don't have the underlying ToolCall.id from execute_plan, so we
            // synthesise per-pair ids from the plan step id.
            let pid = n3ur0n_core::InstanceId::parse(&entry.peer_id)
                .unwrap_or_else(|_| node.instance_id());
            let call_id = state.push_tool_call(
                pid.clone(),
                entry.capability.clone(),
                entry.args.clone(),
            );
            persist_last(node.db(), state)
                .map_err(|e| NodeError::InvalidPayload(format!("persist tool_call: {e}")))?;
            state.push_tool_result(
                call_id,
                pid,
                entry.capability.clone(),
                entry.result.clone(),
                entry.error.clone(),
            );
            persist_last(node.db(), state)
                .map_err(|e| NodeError::InvalidPayload(format!("persist tool_result: {e}")))?;
        }

        // 6. Reflect.
        self.reflect_only(
            node,
            state,
            &planner_text,
            Some(&run.blackboard_summary()),
            run.trace,
            events,
        )
        .await
    }
}

/// Helper: reflect on the original user prompt + (optional) blackboard
/// summary, persist assistant turn, return outcome.
impl PlanExecPlanner {
    async fn reflect_only(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: &str,
        blackboard_summary: Option<&str>,
        trace: Vec<TraceEntry>,
        events: Option<&EventSender>,
    ) -> NodeResult<DispatchOutcome> {
        if let Some(tx) = events {
            let _ = tx.send(DispatchEvent::Reflecting);
        }
        let mut messages: Vec<Value> = Vec::with_capacity(MAX_CONTEXT_TURNS + 2);
        messages.push(json!({"role": "system", "content": self.reflect_system_prompt()}));
        // Include the conversation tail so the LLM has continuity.
        messages.extend(state.to_chat_messages(MAX_CONTEXT_TURNS));
        if let Some(summary) = blackboard_summary {
            messages.push(json!({
                "role": "system",
                "content": format!("Blackboard from this dispatch:\n{}", summary)
            }));
        }
        // Re-state the user's request so the model anchors on it.
        messages.push(json!({"role": "user", "content": user_message}));

        let mut args = json!({
            "messages": messages,
            "temperature": 0.2,
        });
        if let Some(model) = &self.model_hint {
            args["model"] = Value::String(model.clone());
        }
        let response = self.llm_backend.invoke("chat", args).await?;
        let content = response
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let model_used = response
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.model_hint.clone());

        state.push_assistant(content.clone(), model_used.clone());
        persist_last(node.db(), state)
            .map_err(|e| NodeError::InvalidPayload(format!("persist assistant: {e}")))?;

        if let Some(tx) = events {
            let _ = tx.send(DispatchEvent::Final {
                reply: content.clone(),
                model: model_used.clone(),
            });
        }

        Ok(DispatchOutcome {
            reply: content,
            model: model_used,
            trace,
        })
    }
}

impl crate::planner::plan::PlanRun {
    /// Pretty short summary suitable for system context: one line per step.
    pub fn blackboard_summary(&self) -> String {
        let mut out = String::new();
        for entry in &self.trace {
            let value_str = entry
                .result
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "?".into()))
                .unwrap_or_else(|| {
                    entry
                        .error
                        .clone()
                        .map(|e| format!("ERROR: {e}"))
                        .unwrap_or_else(|| "(no result)".into())
                });
            out.push_str(&format!(
                "- {}::{} → {}\n",
                short(&entry.peer_id),
                entry.capability,
                value_str
            ));
        }
        out
    }
}

/// Canonical compile system prompt — moved out of `PlanExecPlanner` so
/// `LocalLLMCompiler` can share it as the default. Pure function of the
/// catalog; no planner state involved.
pub fn default_compile_system_prompt(catalog: &Catalog) -> String {
    let mut s = String::from(
        "You are an n3ur0n plan compiler. Given a user request, produce ONE JSON \
plan that a deterministic executor will run. Output ONLY valid JSON conforming to \
this schema:\n\n\
{\n\
  \"plan\": [\n\
    {\n\
      \"id\":         \"<short alpha-numeric id, unique>\",\n\
      \"peer\":       \"<short_peer from the skills list>\",\n\
      \"capability\": \"<capability name from the skills list>\",\n\
      \"args\":       { ...skill-specific args... },\n\
      \"depends_on\": [ \"<other step ids>\", ... ]\n\
    },\n\
    ...\n\
  ]\n\
}\n\n\
Structural rules (independent of which skills exist):\n\
- Return ONLY the JSON; no prose, no Markdown fences.\n\
- Reference results from earlier steps inside `args` with the exact syntax \
`${stepid.path.to.value}` — the dollar sign is required. Example:\n\
    {\"id\": \"s1\", \"peer\": \"abc\", \"capability\": \"random_int\", \"args\": {\"min\":1,\"max\":10}}\n\
    {\"id\": \"s2\", \"peer\": \"abc\", \"capability\": \"reverse\", \"args\": {\"text\": \"${s1.value}\"}}\n\
    {\"id\": \"s3\", \"peer\": \"xyz\", \"capability\": \"chat\",\n\
     \"args\": {\"prompt\": \"Write one rhyming line about ${s2.reversed}\"}}\n\
- A reference inside args creates an implicit dependency (omit from `depends_on`).\n\
- NO arithmetic, conditionals, or function calls inside `${...}`. Only paths.\n\
  WRONG: `${s2.value + s1.year}`, `${len(s1.text)}`, `${s1.value * 2}`.\n\
  RIGHT: include the raw values as separate refs and let the downstream skill \
  do the math. Example: `\"prompt\": \"Year ${s1.year} plus ${s2.value} — describe \
a bicycle from that year.\"`.\n\
- Each `${...}` head must match a step id you defined earlier in the plan. \
Do NOT invent step ids.\n\
- For chat-like skills, set only fields the schema declares; never set `model`.\n\
- All `peer` values must come from the skills list verbatim.\n\
- Pick the SHORTEST useful plan. If the user asks something you can answer from \
prior knowledge with no skill, return an empty plan: `{\"plan\": []}`. The \
reflection step that runs after execution will compose the answer using your \
own knowledge.\n\
- Tasks that should return `{\"plan\": []}` include: translation between human \
languages, definitions, well-known facts, simple arithmetic, code explanations, \
summaries of text the user already provided. Do not invent a chain of skills \
just because they are listed.\n\
- A skill is RELEVANT only when its declared description and examples match the \
user's intent. When in doubt, prefer fewer steps. Skill-specific semantics live \
in the skill metadata below — read it.\n\
- Do NOT add filler steps that only relay a previous result. The reflection \
step at the end already turns the blackboard into the user's reply.\n\
\n\
Available skills (each entry: name — description, schema, examples, \
disambiguation, anti-patterns):\n\n",
    );
    if catalog.is_empty() {
        s.push_str("(none)\n");
    } else {
        for t in &catalog.tools {
            s.push_str(&render_skill_block(catalog, t));
        }
    }
    s
}

/// Render one capability as a multi-line block for the compile prompt.
/// Each block carries the planner-oriented metadata (examples,
/// disambiguation, negative_examples, output_semantic) so the LLM has the
/// information it needs to match intent → skill without the planner code
/// having to bake skill-specific rules into the system prompt.
fn render_skill_block(catalog: &Catalog, t: &ToolDef) -> String {
    let cap = &t.cap;
    let name = catalog.tool_name(t);
    let schema_in = serde_json::to_string(&cap.schema_in)
        .unwrap_or_else(|_| "{}".into());

    let mut out = String::new();
    out.push_str(&format!("## {name}\n"));
    out.push_str(&format!("description: {}\n", cap.description));
    out.push_str(&format!("schema_in: {schema_in}\n"));

    // Up to 2 examples — enough to seed pattern match, not so many we
    // crowd the context window.
    if !cap.examples.is_empty() {
        out.push_str("examples:\n");
        for ex in cap.examples.iter().take(2) {
            let args = serde_json::to_string(&ex.args)
                .unwrap_or_else(|_| "{}".into());
            out.push_str(&format!(
                "  - intent: \"{}\" → args: {}\n",
                ex.user_intent, args
            ));
        }
    }
    if let Some(disambig) = &cap.disambiguation {
        out.push_str(&format!("disambiguation: {disambig}\n"));
    }
    if !cap.negative_examples.is_empty() {
        out.push_str("do_NOT_use_for:\n");
        for ne in cap.negative_examples.iter().take(2) {
            out.push_str(&format!(
                "  - intent: \"{}\" — {}\n",
                ne.user_intent, ne.why_not
            ));
        }
    }
    if let Some(sem) = &cap.output_semantic {
        out.push_str(&format!("output_means: {sem}\n"));
    }
    out.push('\n');
    out
}

fn short(peer_id: &str) -> String {
    let trimmed = peer_id.strip_prefix("n3:").unwrap_or(peer_id);
    trimmed.chars().take(12).collect()
}

/// Parse a JSON plan with two fallbacks:
/// 1. Direct serde parse (LLM emits exactly the schema).
/// 2. Look for the first `{` ... matching `}` and try parsing that.
pub(crate) fn parse_plan(raw: &str) -> Result<Plan, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty plan response".into());
    }

    // Strip Markdown fences if present.
    let cleaned = strip_md_fences(trimmed);

    if let Ok(p) = serde_json::from_str::<Plan>(cleaned) {
        return Ok(p);
    }

    // Try to extract the first JSON object substring.
    if let Some(json_str) = extract_first_json_object(cleaned) {
        if let Ok(p) = serde_json::from_str::<Plan>(&json_str) {
            return Ok(p);
        }
    }

    Err(format!(
        "could not parse Plan from response: {}",
        cleaned.chars().take(120).collect::<String>()
    ))
}

fn strip_md_fences(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with("```") {
        // strip first line of fence and trailing fence
        if let Some(after_first) = s.find('\n') {
            let body = &s[after_first + 1..];
            if let Some(end) = body.rfind("```") {
                return body[..end].trim();
            }
            return body.trim();
        }
    }
    s
}

fn extract_first_json_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_core::capability::{
        AccessMode, CapabilityDecl, CapabilityExample, NegativeExample,
    };

    #[test]
    fn parse_plan_direct() {
        let raw = r#"{"plan":[{"id":"s1","peer":"p","capability":"c","args":{}}]}"#;
        let p = parse_plan(raw).unwrap();
        assert_eq!(p.plan.len(), 1);
    }

    #[test]
    fn parse_plan_with_fences() {
        let raw = "```json\n{\"plan\":[]}\n```";
        let p = parse_plan(raw).unwrap();
        assert_eq!(p.plan.len(), 0);
    }

    #[test]
    fn parse_plan_with_prefix_text() {
        let raw = "Here you go:\n{\"plan\":[{\"id\":\"s1\",\"peer\":\"p\",\"capability\":\"c\",\"args\":{}}]}\nDone.";
        let p = parse_plan(raw).unwrap();
        assert_eq!(p.plan.len(), 1);
    }

    fn enriched_cap() -> CapabilityDecl {
        CapabilityDecl {
            name: "reverse".into(),
            description: "Reverses a string char by char.".into(),
            schema_in: json!({"type": "object", "required": ["text"]}),
            schema_out: json!({"type": "object"}),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec![],
            lobe_ids: vec![],
            examples: vec![CapabilityExample {
                user_intent: "reverse 'hello'".into(),
                args: json!({"text": "hello"}),
                expected_output: json!({"reversed": "olleh"}),
            }],
            disambiguation: Some("Char-level only, not translation.".into()),
            negative_examples: vec![NegativeExample {
                user_intent: "translate to French".into(),
                why_not: "use chat cap instead".into(),
            }],
            output_semantic: Some("input string reversed".into()),
            version: "0.0.0".into(),
            languages: vec![],
            countries: vec![],
        }
    }

    #[test]
    fn skill_block_renders_metadata() {
        let mut cat = Catalog::default();
        cat.tools.push(ToolDef {
            peer_id: "n3:abcdef123456ghi".into(),
            peer_endpoint: Some("http://x".into()),
            cap: enriched_cap(),
        });
        let block = render_skill_block(&cat, &cat.tools[0]);
        assert!(block.contains("## abcdef123456::reverse"));
        assert!(block.contains("description: Reverses"));
        assert!(block.contains("examples:"));
        assert!(block.contains("reverse 'hello'"));
        assert!(block.contains("disambiguation: Char-level"));
        assert!(block.contains("do_NOT_use_for:"));
        assert!(block.contains("translate to French"));
        assert!(block.contains("output_means: input string reversed"));
    }
}
