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
use time::OffsetDateTime;
use tracing::{debug, warn};

use crate::conversation::{ConversationState, persist_last, persist_tool_pair_at};
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::catalog::{Catalog, ToolDef};
use crate::planner::compiler::PlanCompiler;
use crate::planner::plan::{Plan, execute_plan_streaming, validate_plan};
use crate::planner::retriever::Retriever;
use crate::planner::{
    DispatchEvent, DispatchMode, DispatchOptions, DispatchOutcome, EventSender, MAX_CONTEXT_TURNS,
    PlanStepInfo, Planner, TraceEntry,
};
/// Maximum number of *remote* tools surfaced in the compile prompt. Local
/// tools always pass through (the operator configured them explicitly).
/// 20 picked to keep prompts under ~3k tokens for moderately enriched
/// caps; tune if observed compile latency starts to dominate.
pub const REMOTE_TOP_K: usize = 20;

/// How many corrective recompiles a rejected plan gets. One.
///
/// The one-shot compile gives up the recovery loop a ReAct agent leans
/// on: a plan that fails `validate_plan` is discarded whole and the user
/// gets a no-tool answer. A single retry carrying the validator's own
/// error message buys part of that loop back for one extra LLM call
/// *only on failure* — the success path stays at two calls. More than
/// one retry is not worth the latency: if the model cannot fix a plan
/// given the exact error, another identical nudge rarely helps.
const COMPILE_RETRY_LIMIT: usize = 1;

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
    /// Capability retrieval. Defaults to BM25-only; `with_retriever`
    /// swaps in a hybrid one when an embeddings endpoint is configured.
    pub retriever: Arc<Retriever>,
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
            retriever: Arc::new(Retriever::lexical()),
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
            retriever: Arc::new(Retriever::lexical()),
        }
    }

    /// Swap in a retriever — used to enable hybrid (BM25 + embedding)
    /// capability retrieval when the node has an embeddings endpoint.
    pub fn with_retriever(mut self, retriever: Arc<Retriever>) -> Self {
        self.retriever = retriever;
        self
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
        self.dispatch_inner(node, state, input, Some(&events)).await
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

        // 2. Build catalog, then rank and bound it against the user
        //    message so prompt size stays bounded as the network grows.
        //    Scoring is the retriever's job (BM25, plus embeddings when
        //    configured); ranking policy is the catalog's.
        let registry_snapshot = node.registry();
        let catalog = Catalog::build(
            node.instance_id().as_str(),
            &registry_snapshot,
            node.db(),
            500,
        )?;
        let scores = self.retriever.score(&catalog.tools, &planner_text).await;
        let catalog = catalog.filter_with_scores(&scores, REMOTE_TOP_K);

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
        if confidence < 0.5
            && let Some(tx) = events
        {
            let _ = tx.send(DispatchEvent::LowConfidence { confidence });
        }

        // 4. Validate, with one corrective recompile if the plan is
        //    structurally wrong (see `resolve_plan`).
        let plan = match resolve_plan(self.compiler.as_ref(), &planner_text, &catalog, plan).await {
            // Empty plan = answer directly without any tool. This is a
            // legitimate, prompt-instructed outcome (translation,
            // arithmetic, definitions…), never a retry trigger.
            PlanOutcome::Empty => {
                if let Some(tx) = events {
                    let _ = tx.send(DispatchEvent::PlanReady { steps: Vec::new() });
                }
                return self
                    .reflect_only(node, state, &planner_text, None, Vec::new(), events)
                    .await;
            }
            PlanOutcome::Invalid(e) => {
                warn!(error = %e, "plan still invalid after retry; falling back to direct reply");
                if let Some(tx) = events {
                    let _ = tx.send(DispatchEvent::PlanReady { steps: Vec::new() });
                }
                return self
                    .reflect_only(node, state, &planner_text, None, Vec::new(), events)
                    .await;
            }
            PlanOutcome::Valid(p) => p,
        };

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
        //
        // Open a `plan_runs` journal row (status=running) before execution so a
        // crash mid-run is detectable as an orphan. The durability hook then
        // persists each step's tool pair the moment it completes, at a seq
        // reserved by its plan index — so a partial trace stays correctly
        // ordered. The in-memory ConversationState is reconciled afterwards
        // from the returned trace (and is NOT persisted again here).
        let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
        let started_at = OffsetDateTime::now_utc().unix_timestamp();
        let plan_json = serde_json::to_string(&plan)
            .map_err(|e| NodeError::InvalidPayload(format!("serialize plan: {e}")))?;
        n3ur0n_storage::plan_runs::insert(
            node.db(),
            &n3ur0n_storage::plan_runs::PlanRunRecord {
                id: run_id.clone(),
                conversation_id: state.id.clone(),
                plan_json,
                status: "running".into(),
                created_at: started_at,
                finished_at: None,
            },
        )
        .map_err(|e| NodeError::InvalidPayload(format!("insert plan_run: {e}")))?;

        // User turn sits at `next_seq() - 1`; the reserved tool block starts
        // right after it. Stamp every tool turn with the dispatch start time.
        let base_seq = state.next_seq() - 1;
        let db_hook = node.db().clone();
        let conv_id_hook = state.id.clone();
        // Output blobs are indexed against the conversation's owner, otherwise
        // they never surface in that user's Files panel.
        let blob_owner = crate::blob_resolve::BlobOwner {
            client_id: Some(state.client_id.clone()),
            conversation_id: Some(state.id.clone()),
        };
        let fallback_id = node.instance_id();
        let mut on_done = |idx: usize, entry: &TraceEntry| {
            let peer = n3ur0n_core::InstanceId::parse(&entry.peer_id)
                .unwrap_or_else(|_| fallback_id.clone());
            if let Err(e) = persist_tool_pair_at(
                &db_hook,
                &conv_id_hook,
                base_seq,
                idx,
                &entry.call_id,
                &peer,
                &entry.capability,
                &entry.args,
                &entry.result,
                &entry.error,
                started_at,
            ) {
                warn!(error = %e, step = idx, "failed to persist tool turn mid-execution");
            }
        };

        let run = match execute_plan_streaming(
            node,
            &plan,
            &catalog,
            events,
            Some(&mut on_done),
            &blob_owner,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let finished_at = OffsetDateTime::now_utc().unix_timestamp();
                if let Err(se) =
                    n3ur0n_storage::plan_runs::set_status(node.db(), &run_id, "failed", finished_at)
                {
                    warn!(error = %se, "failed to mark plan_run failed");
                }
                return Err(e);
            }
        };

        // Reconcile the in-memory conversation with the executed trace. The DB
        // already holds these turns (written incrementally by the hook above);
        // here we only mirror them into the cached state, reusing each step's
        // `call_id` so both views agree. No re-persist.
        for entry in &run.trace {
            let pid = n3ur0n_core::InstanceId::parse(&entry.peer_id)
                .unwrap_or_else(|_| node.instance_id());
            state.push_tool_call_with_id(
                entry.call_id.clone(),
                pid.clone(),
                entry.capability.clone(),
                entry.args.clone(),
            );
            state.push_tool_result(
                entry.call_id.clone(),
                pid,
                entry.capability.clone(),
                entry.result.clone(),
                entry.error.clone(),
            );
        }

        // 6. Reflect.
        let outcome = self
            .reflect_only(
                node,
                state,
                &planner_text,
                Some(&run.blackboard_summary()),
                run.trace,
                events,
            )
            .await;
        let finished_at = OffsetDateTime::now_utc().unix_timestamp();
        let status = if outcome.is_ok() { "done" } else { "failed" };
        if let Err(e) =
            n3ur0n_storage::plan_runs::set_status(node.db(), &run_id, status, finished_at)
        {
            warn!(error = %e, "failed to close plan_run journal row");
        }
        outcome
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
            let value_str = match (&entry.result, &entry.error) {
                (Some(v), _) => render_blackboard_value(v),
                (None, Some(e)) => format!("ERROR: {e}"),
                (None, None) => "(no result)".into(),
            };
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

/// Per-step cap on the reflect-prompt blackboard rendering (~1 KB). A single
/// step returning a large document otherwise saturates a 7B model's context.
const BLACKBOARD_ENTRY_CAP: usize = 1024;

/// Render one step result for the reflect prompt under [`BLACKBOARD_ENTRY_CAP`].
///
/// If the result carries blob references (P2), surface the reference itself —
/// hash, size, mime — and never the binary/large content the blob stands for.
/// Otherwise serialise and defensively truncate on a char boundary.
fn render_blackboard_value(v: &Value) -> String {
    let refs = crate::blob_resolve::collect_blob_refs(v);
    if !refs.is_empty() {
        return refs
            .iter()
            .map(|b| format!("blob {} ({} B, {})", b.hash, b.size, b.mime))
            .collect::<Vec<_>>()
            .join("; ");
    }
    let s = serde_json::to_string(v).unwrap_or_else(|_| "?".into());
    if s.len() <= BLACKBOARD_ENTRY_CAP {
        return s;
    }
    let total = s.len();
    let mut end = BLACKBOARD_ENTRY_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated, {total} bytes]", &s[..end])
}

/// Canonical compile system prompt — moved out of `PlanExecPlanner` so
/// `LocalLLMCompiler` can share it as the default. Pure function of the
/// catalog; no planner state involved.
/// What a compiled plan turned out to be, after validation and at most
/// [`COMPILE_RETRY_LIMIT`] corrective recompiles.
#[derive(Debug)]
pub enum PlanOutcome {
    /// No steps — the model chose to answer from its own knowledge. A
    /// legitimate outcome the compile prompt explicitly asks for, not a
    /// failure, so it never triggers a retry.
    Empty,
    /// Structurally sound against the catalog.
    Valid(Plan),
    /// Still rejected after the retry budget; carries the last error.
    Invalid(String),
}

/// Validate `plan`, and on a structural failure recompile once with the
/// validator's own error fed back to the model.
///
/// Split out of `dispatch_inner` so the retry can be unit-tested against
/// a stub `PlanCompiler` — exercising it through `dispatch_inner` would
/// need a whole `Node` (db, keypair, registry).
///
/// The empty-plan check comes **before** validation on purpose.
/// `validate_plan` reports an empty plan as `"plan has no steps"`, so
/// validating first would treat every legitimate no-tool answer as a
/// failure and burn a second LLM call on the most common path.
pub async fn resolve_plan(
    compiler: &dyn PlanCompiler,
    user_msg: &str,
    catalog: &Catalog,
    plan: Plan,
) -> PlanOutcome {
    if plan.plan.is_empty() {
        return PlanOutcome::Empty;
    }
    let Err(first) = validate_plan(&plan, catalog) else {
        return PlanOutcome::Valid(plan);
    };

    let mut last = first.to_string();
    for attempt in 1..=COMPILE_RETRY_LIMIT {
        warn!(
            error = %last,
            attempt,
            "plan rejected; recompiling once with the validation error fed back"
        );
        let retried = match compiler
            .compile(&compile_retry_message(user_msg, &last), catalog)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                // The retry call itself failed (backend down, timeout).
                // Report the original validation error — it is the more
                // useful diagnostic — but note the retry never landed.
                warn!(error = %e, "corrective recompile failed to reach the backend");
                return PlanOutcome::Invalid(last);
            }
        };
        // The model may conclude on the second pass that no skill fits.
        // That is a valid answer, not a second failure.
        if retried.plan.is_empty() {
            return PlanOutcome::Empty;
        }
        match validate_plan(&retried, catalog) {
            Ok(()) => {
                debug!(attempt, "corrective recompile produced a valid plan");
                return PlanOutcome::Valid(retried);
            }
            Err(e) => last = e.to_string(),
        }
    }
    PlanOutcome::Invalid(last)
}

/// Build the corrective user message: the original request, the
/// validator's verbatim complaint, and the two ways out.
///
/// The error is fed back verbatim because `validate_plan`'s messages are
/// already actionable and name the offending step — e.g. ``step `s2`:
/// tool `abc::x` not in catalog``.
///
/// **Order matters here, and it is not cosmetic.** An earlier draft led
/// with "emit a corrected plan" and offered the empty plan as a
/// footnote; measured against the eval suite that rescued a `none` case
/// (`code_explain`) into executing a tool it never should have called.
/// A rejected plan is frequently a plan that should not have existed:
/// on the `none` / `trap` categories the validation failure was acting
/// as an accidental safety net, and a retry framed as "fix it" defeats
/// that net. Reconsidering therefore comes first and correcting second,
/// so the model is not anchored on producing a plan at any cost.
fn compile_retry_message(user_msg: &str, error: &str) -> String {
    format!(
        "{user_msg}\n\n\
         [plan rejected] Your previous plan was rejected by the validator:\n\
         {error}\n\
         \n\
         First reconsider whether any skill is needed at all. A rejected plan is \
         often a plan that should not exist: if this request can be answered from \
         your own knowledge — translation, arithmetic, definitions, explaining code \
         or text the user already provided — return {{\"plan\": []}}. That is the \
         correct answer, not a failure.\n\
         Only if a skill genuinely is required, emit a corrected plan: use the exact \
         `peer:` and `capability:` values from the skills list above, and only \
         argument fields the skill's schema declares."
    )
}

pub fn default_compile_system_prompt(catalog: &Catalog) -> String {
    let mut s = String::from(
        "You are an n3ur0n plan compiler. Given a user request, produce ONE JSON \
plan that a deterministic executor will run. Output ONLY valid JSON conforming to \
this schema:\n\n\
{\n\
  \"plan\": [\n\
    {\n\
      \"id\":         \"<short alpha-numeric id, unique>\",\n\
      \"peer\":       \"<the `peer:` value of the chosen skill — NOT the `##` header>\",\n\
      \"capability\": \"<the `capability:` value of the chosen skill>\",\n\
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
- Decide this against the skills that are actually listed below, not against \
the kind of task it is. If a listed skill is DEDICATED to what the user asked \
(a translation skill for a translation, a summarising skill for a summary), \
use it — it is more accurate than answering from memory. A general-purpose \
chat or LLM skill is NOT dedicated to anything: never route to one for \
something you can answer yourself. If nothing listed is dedicated to the \
request and you can answer from your own knowledge — greetings, definitions, \
well-known facts, simple arithmetic, explaining code or text the user already \
gave you — return `{\"plan\": []}`. Do not invent a chain of skills just \
because they are listed.\n\
- BUT you must NEVER answer from memory when the request needs live or current \
data you cannot possibly know from training — above all the CURRENT TIME, the \
time \"now\", or today's DATE. You do not know the current time. If a skill \
provides it (e.g. a `time` skill), you MUST use it; do not fabricate a time. \
This applies even when the time is only an input to another step (e.g. \
\"show the current time reversed\" → get the time, then reverse it).\n\
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
    // `tool_name` is `<short_peer>::<capability>`. Present the two parts on
    // *separate* labelled lines: a combined `peer::cap` header led small models
    // to copy the whole string into the plan's `peer` field, producing an
    // unresolvable `peer::cap::cap` tool name.
    let full = catalog.tool_name(t);
    let peer = full
        .split_once("::")
        .map(|(p, _)| p)
        .unwrap_or(full.as_str());
    let schema_in = serde_json::to_string(&cap.schema_in).unwrap_or_else(|_| "{}".into());

    let mut out = String::new();
    out.push_str(&format!("## {}\n", cap.name));
    out.push_str(&format!("peer: {peer}\n"));
    out.push_str(&format!("capability: {}\n", cap.name));
    out.push_str(&format!("description: {}\n", cap.description));
    out.push_str(&format!("schema_in: {schema_in}\n"));

    // Output field names, so later steps reference the RIGHT path
    // (`${step.<field>}`). Without this the model guesses (e.g. `${s1.value}`
    // on a step whose output is actually `{now, unix}`), producing an
    // unresolvable reference at execution time.
    if let Some(props) = cap.schema_out.get("properties").and_then(|p| p.as_object())
        && !props.is_empty()
    {
        let fields = props
            .iter()
            .map(|(k, v)| {
                let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                format!("{k} ({ty})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "output_fields: {fields}   (reference as ${{<step_id>.<field>}})\n"
        ));
    }

    // Up to 2 examples — enough to seed pattern match, not so many we
    // crowd the context window.
    if !cap.examples.is_empty() {
        out.push_str("examples:\n");
        for ex in cap.examples.iter().take(2) {
            let args = serde_json::to_string(&ex.args).unwrap_or_else(|_| "{}".into());
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
    if let Some(json_str) = extract_first_json_object(cleaned)
        && let Ok(p) = serde_json::from_str::<Plan>(&json_str)
    {
        return Ok(p);
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
    use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample, NegativeExample};

    // ---- retry harness -------------------------------------------------
    //
    // A stub compiler returning canned plans, so `resolve_plan` can be
    // driven without a `Node`. It records the messages it was handed,
    // which is how the tests assert the validation error is actually fed
    // back rather than the retry being a blind second roll of the dice.

    #[derive(Debug, Default)]
    struct StubCompiler {
        replies: std::sync::Mutex<std::collections::VecDeque<NodeResult<Plan>>>,
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl StubCompiler {
        fn with(replies: Vec<NodeResult<Plan>>) -> Self {
            Self {
                replies: std::sync::Mutex::new(replies.into()),
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl PlanCompiler for StubCompiler {
        async fn compile(&self, user_msg: &str, _catalog: &Catalog) -> NodeResult<Plan> {
            self.seen.lock().unwrap().push(user_msg.to_string());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(Plan { plan: vec![] }))
        }
    }

    fn retry_catalog() -> Catalog {
        let mut cat = Catalog::default();
        cat.tools.push(ToolDef {
            peer_id: "n3:abcdef123456".into(),
            peer_endpoint: None,
            cap: enriched_cap(),
        });
        cat
    }

    /// A step naming a tool the catalog does not contain.
    fn bad_plan() -> Plan {
        Plan {
            plan: vec![crate::planner::plan::PlanStep {
                id: "s1".into(),
                peer: "ghostpeer000".into(),
                capability: "nope".into(),
                args: serde_json::json!({}),
                depends_on: vec![],
            }],
        }
    }

    fn good_plan(cat: &Catalog) -> Plan {
        let t = &cat.tools[0];
        let full = cat.tool_name(t);
        let (peer, cap) = full.split_once("::").unwrap();
        Plan {
            plan: vec![crate::planner::plan::PlanStep {
                id: "s1".into(),
                peer: peer.into(),
                capability: cap.into(),
                args: serde_json::json!({"text": "hello"}),
                depends_on: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn valid_plan_is_not_recompiled() {
        let cat = retry_catalog();
        let stub = StubCompiler::default();
        let out = resolve_plan(&stub, "hi", &cat, good_plan(&cat)).await;
        assert!(matches!(out, PlanOutcome::Valid(_)), "got {out:?}");
        assert!(
            stub.calls().is_empty(),
            "success path must not call the LLM again"
        );
    }

    /// The most common no-tool path. `validate_plan` reports an empty
    /// plan as an error, so validating before the empty check would burn
    /// a second LLM call on every translation / arithmetic / definition.
    #[tokio::test]
    async fn empty_plan_is_not_a_failure_and_never_retries() {
        let cat = retry_catalog();
        let stub = StubCompiler::default();
        let out = resolve_plan(&stub, "translate this", &cat, Plan { plan: vec![] }).await;
        assert!(matches!(out, PlanOutcome::Empty), "got {out:?}");
        assert!(
            stub.calls().is_empty(),
            "empty plan must not trigger a retry"
        );
    }

    #[tokio::test]
    async fn invalid_plan_recompiles_once_and_recovers() {
        let cat = retry_catalog();
        let stub = StubCompiler::with(vec![Ok(good_plan(&cat))]);
        let out = resolve_plan(&stub, "do the thing", &cat, bad_plan()).await;
        assert!(matches!(out, PlanOutcome::Valid(_)), "got {out:?}");

        let calls = stub.calls();
        assert_eq!(calls.len(), 1, "exactly one corrective recompile");
        // The retry must carry the original request *and* the validator's
        // complaint, else it is just a blind re-roll.
        assert!(calls[0].contains("do the thing"));
        assert!(calls[0].contains("[plan rejected]"));
        assert!(
            calls[0].contains("not in catalog"),
            "verbatim error: {}",
            calls[0]
        );
        assert!(calls[0].contains(r#"{"plan": []}"#), "must allow giving up");
    }

    #[tokio::test]
    async fn retry_budget_is_one() {
        let cat = retry_catalog();
        // Both the first plan and the retry are bad.
        let stub = StubCompiler::with(vec![Ok(bad_plan())]);
        let out = resolve_plan(&stub, "q", &cat, bad_plan()).await;
        match out {
            PlanOutcome::Invalid(e) => assert!(e.contains("not in catalog"), "got {e}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
        assert_eq!(
            stub.calls().len(),
            COMPILE_RETRY_LIMIT,
            "no unbounded looping"
        );
    }

    /// A model that reconsiders and returns `{"plan": []}` on the second
    /// pass is answering, not failing twice.
    #[tokio::test]
    async fn retry_may_conclude_no_tool_fits() {
        let cat = retry_catalog();
        let stub = StubCompiler::with(vec![Ok(Plan { plan: vec![] })]);
        let out = resolve_plan(&stub, "q", &cat, bad_plan()).await;
        assert!(matches!(out, PlanOutcome::Empty), "got {out:?}");
    }

    /// If the retry call cannot reach the backend, report the original
    /// validation error — the transport error is not what the operator
    /// needs to see about the plan.
    #[tokio::test]
    async fn backend_failure_during_retry_reports_the_validation_error() {
        let cat = retry_catalog();
        let stub = StubCompiler::with(vec![Err(NodeError::InvalidPayload("upstream down".into()))]);
        let out = resolve_plan(&stub, "q", &cat, bad_plan()).await;
        match out {
            PlanOutcome::Invalid(e) => {
                assert!(e.contains("not in catalog"), "got {e}");
                assert!(!e.contains("upstream down"), "transport error leaked: {e}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

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
    fn compile_prompt_size_is_bounded() {
        // Measure the assembled compile prompt for a small catalog and guard
        // against prompt bloat. Prints the size so we can eyeball headroom vs a
        // modest served context (Ollama default ~4K). Run with --nocapture.
        let mut cat = Catalog::default();
        for i in 0..6 {
            cat.tools.push(ToolDef {
                peer_id: format!("n3:peer{i}00000000000000000000000000"),
                peer_endpoint: Some("http://x".into()),
                cap: enriched_cap(),
            });
        }
        let prompt = default_compile_system_prompt(&cat);
        let approx_tokens = prompt.chars().count() / 4;
        println!(
            "compile prompt (6 caps): {} chars, ~{approx_tokens} tokens",
            prompt.len()
        );
        // ~6K-token budget leaves headroom under an 8K context; 6 rich caps must
        // stay well under it.
        assert!(
            approx_tokens < 6000,
            "compile prompt ~{approx_tokens} tokens exceeds budget"
        );
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
        // peer and capability are on separate labelled lines (not a combined
        // `peer::cap` header) so small models don't conflate them.
        assert!(block.contains("## reverse"));
        assert!(block.contains("peer: abcdef123456"));
        assert!(block.contains("capability: reverse"));
        assert!(block.contains("description: Reverses"));
        assert!(block.contains("examples:"));
        assert!(block.contains("reverse 'hello'"));
        assert!(block.contains("disambiguation: Char-level"));
        assert!(block.contains("do_NOT_use_for:"));
        assert!(block.contains("translate to French"));
        assert!(block.contains("output_means: input string reversed"));
    }

    // --- P2: context control in blackboard_summary -------------------------

    fn trace_entry(cap: &str, result: Value) -> TraceEntry {
        TraceEntry {
            call_id: "call".into(),
            peer_id: "n3:abcdef123456ghi".into(),
            capability: cap.into(),
            args: json!({}),
            result: Some(result),
            error: None,
        }
    }

    fn plan_run(trace: Vec<TraceEntry>) -> crate::planner::plan::PlanRun {
        crate::planner::plan::PlanRun {
            blackboard: std::collections::HashMap::new(),
            last_step_id: None,
            trace,
        }
    }

    #[test]
    fn blackboard_summary_truncates_large_result() {
        let big = json!({ "data": "x".repeat(100_000) });
        let run = plan_run(vec![trace_entry("bulk", big)]);
        let s = run.blackboard_summary();
        // One entry capped at ~1 KB + header/suffix — never the full 100 KB.
        assert!(s.len() < 4_000, "summary not bounded: {} bytes", s.len());
        assert!(s.contains("truncated"), "missing truncation marker: {s}");
    }

    #[test]
    fn blackboard_summary_renders_blobref_not_content() {
        let hash = format!("sha256:{}", "a".repeat(64));
        let result = json!({
            "blob": { "hash": hash, "size": 999_999, "mime": "application/pdf" },
            "noise": "z".repeat(50_000),
        });
        let run = plan_run(vec![trace_entry("gen", result)]);
        let s = run.blackboard_summary();
        assert!(s.contains(&hash), "blob hash absent: {s}");
        assert!(s.contains("application/pdf"), "blob mime absent: {s}");
        // The bulky sibling content must never reach the prompt.
        assert!(
            !s.contains(&"z".repeat(2_000)),
            "blob-bearing result leaked content"
        );
    }

    #[test]
    fn blackboard_summary_preserves_small_results() {
        let run = plan_run(vec![trace_entry("calc", json!({ "sum": 42 }))]);
        let s = run.blackboard_summary();
        assert!(s.contains("\"sum\":42"), "small result altered: {s}");
        assert!(!s.contains("truncated"), "small result wrongly truncated");
    }
}
