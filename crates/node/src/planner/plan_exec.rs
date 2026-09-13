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

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::Backend;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tracing::{debug, warn};

use crate::conversation::{ConversationState, persist_last, persist_tool_pair_at};
use crate::error::{NodeError, NodeResult};
use crate::mention::MentionScope;
use crate::node::Node;
use crate::planner::catalog::{Catalog, ToolDef};
use crate::planner::compiler::PlanCompiler;
use crate::planner::plan::{Plan, execute_plan_streaming, plan_depth, validate_plan};
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

/// Compile+execute rounds a single dispatch may use.
///
/// One round is today's behaviour: compile a plan, run it, reflect. A second
/// round lets the planner react to what the first one produced — the cheapest
/// form of adaptivity, and the only one that does not put an LLM call between
/// every step.
///
/// The cost profile is the point: a request that compiles a complete plan pays
/// exactly what it paid before. Only a dispatch that trips a trigger pays more.
const MAX_PLAN_ROUNDS: usize = 2;

/// Depth (longest chain of dependent steps) above which a plan is *suspected*
/// of having been cut short, and a continuation is offered.
///
/// Named as a threshold rather than a cap because it is one: nothing rejects a
/// deeper plan. `validate_plan`, the grammar and the compile prompt are all
/// silent about depth, so a plan of depth 8 is accepted and executed in full —
/// it merely also earns a continuation round. The only enforced bound on plan
/// size is `MAX_PLAN_STEPS`.
const CONTINUATION_DEPTH_THRESHOLD: usize = 3;

/// Tool invocations a whole dispatch may execute, across every round.
///
/// `MAX_PLAN_STEPS` bounds one plan; without this, two rounds of eight steps
/// meant sixteen invocations, sixteen possible side effects and sixteen
/// possible charges, with nothing naming that ceiling. Depth bounds neither.
const MAX_TOTAL_STEPS: usize = 12;

/// Wall-clock budget after which no further round starts.
///
/// Individual calls already time out (180 s per peer invoke, 120 s per LLM
/// call), but nothing stopped those from accumulating across rounds and
/// concurrency batches. This does not interrupt work in flight; it refuses to
/// begin more.
const DISPATCH_BUDGET_SECS: i64 = 300;

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
- The blackboard is the complete record of what happened. You performed no \
action yourself: you cannot open, display, download, send or modify \
anything. Never write that you did. If the blackboard is empty, nothing \
was done at all — say so.\n\
- If the user asked for a side-effect action (sending email, posting, \
payments, modifying files, calling an external service) and no tool capable \
of performing it ran successfully, say plainly that you cannot perform it. \
Do NOT pretend the action was done.\n\
- Never invent a request the user did not make. If the message does not ask \
for anything, ask what they want instead of assuming.\n\
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

        // 1. Build catalog, then rank and bound it against the user
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
        // 2b. Explicit `@` mentions narrow the catalogue before ranking: when
        //     the user already said where to go, there is nothing to rank.
        let scope = MentionScope::from_text(&input.text);
        let mut resolved_scope = ResolvedScope::default();
        let catalog = if scope.is_empty() {
            catalog
        } else {
            resolved_scope = resolve_scope(node, &catalog, &scope);
            if resolved_scope.narrows() {
                let before = catalog.tools.len();
                let scoped = catalog.scoped_to(
                    &resolved_scope.peer_ids,
                    &resolved_scope.lobes,
                    &resolved_scope.capabilities,
                );
                debug!(
                    peers = ?resolved_scope.peer_ids,
                    lobes = ?resolved_scope.lobes,
                    capabilities = ?resolved_scope.capabilities,
                    unresolved = ?resolved_scope.unresolved,
                    tools_before = before,
                    tools_after = scoped.tools.len(),
                    "catalogue scoped by explicit mentions"
                );
                scoped
            } else {
                debug!(
                    unresolved = ?resolved_scope.unresolved,
                    "every mention was unresolved"
                );
                catalog
            }
        };

        // Addressing something this node does not know is a mistake to point
        // at, not a request to reinterpret. Answering anyway meant silently
        // widening a scope the user had deliberately narrowed: they wrote one
        // peer, the node used another and said nothing. The reflect prompt did
        // ask the model to disclose it, which is the wrong place for a fact
        // the runtime already holds — a 7B model drops the instruction and the
        // user never learns. Refuse before spending a single LLM call.
        if !resolved_scope.unresolved.is_empty() {
            return Err(NodeError::UnresolvedMentions(
                resolved_scope.unresolved.clone(),
            ));
        }

        // Only now is the turn recorded. Resolving first costs one catalogue
        // read and buys a clean refusal: a rejected command leaves no orphan
        // user turn sitting in the thread with no reply under it.
        state.push_user_input(&input);
        persist_last(node.db(), state)
            .map_err(|e| NodeError::InvalidPayload(format!("persist user: {e}")))?;

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
                    let _ = tx.send(DispatchEvent::PlanReady {
                        steps: Vec::new(),
                        round: 1,
                    });
                }
                return self
                    .reflect_only(
                        node,
                        state,
                        &planner_text,
                        None,
                        None,
                        &resolved_scope.unresolved,
                        Vec::new(),
                        1,
                        events,
                    )
                    .await;
            }
            PlanOutcome::Invalid(e) => {
                warn!(error = %e, "plan still invalid after retry; falling back to direct reply");
                if let Some(tx) = events {
                    let _ = tx.send(DispatchEvent::PlanReady {
                        steps: Vec::new(),
                        round: 1,
                    });
                }
                return self
                    .reflect_only(
                        node,
                        state,
                        &planner_text,
                        None,
                        None,
                        &resolved_scope.unresolved,
                        Vec::new(),
                        1,
                        events,
                    )
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
            let _ = tx.send(DispatchEvent::PlanReady { steps, round: 1 });
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

        // 5b. Continuation rounds.
        //
        // Two triggers, both structural — neither asks the model to judge its
        // own completeness, which is the assessment a 7B is worst at:
        //
        //   - a step failed, so the plan cannot have done what it set out to;
        //   - the plan reached the per-round depth cap, so it may have been
        //     cut short by the bound rather than by having finished.
        //
        // The continuation itself is an ordinary compile against the same
        // catalogue, with the blackboard in front of it. An empty plan ends the
        // loop, which is the same signal the first round already uses.
        let mut run = run;
        let mut round = 1usize;
        // Counts compile rounds, not executed plans: a continuation that
        // compiles `{"plan": []}` did happen and cost an LLM call, and it is
        // the only externally visible proof that the loop ran at all.
        let mut rounds_used = 1usize;
        let mut depth = plan_depth(&plan);
        while round < MAX_PLAN_ROUNDS {
            let elapsed = OffsetDateTime::now_utc().unix_timestamp() - started_at;
            let Some(reason) = continuation_reason(&run.trace, depth, elapsed) else {
                break;
            };
            rounds_used += 1;
            // Stamp this round's rows with this round's clock. Reusing the
            // dispatch's `started_at` dated round 2's work minutes before it
            // happened, and disagreed with the in-memory turns, which are
            // stamped when they are reconciled.
            let round_started_at = OffsetDateTime::now_utc().unix_timestamp();
            debug!(round, depth, %reason, "compiling a continuation round");

            let msg = continuation_message(&planner_text, &run.blackboard_summary());
            let compiled = match self.compiler.compile(&msg, &catalog).await {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, round, "continuation compile failed; keeping what we have");
                    break;
                }
            };
            let next = match resolve_plan(self.compiler.as_ref(), &msg, &catalog, compiled).await {
                // Nothing left to do, or nothing valid to do: stop and reflect
                // on what the earlier rounds produced.
                PlanOutcome::Empty => break,
                PlanOutcome::Invalid(e) => {
                    warn!(error = %e, round, "continuation plan invalid; stopping rounds");
                    break;
                }
                PlanOutcome::Valid(p) => p,
            };

            if adds_nothing(&next, &run.trace, &catalog) {
                warn!(
                    round,
                    "continuation only re-proposed capabilities that already ran; \
                     stopping rather than duplicating work and polluting the \
                     blackboard the reply is composed from"
                );
                break;
            }

            if let Some(tx) = events {
                let steps: Vec<PlanStepInfo> = next
                    .plan
                    .iter()
                    .map(|st| {
                        let tool = catalog.find(&format!("{}::{}", st.peer, st.capability));
                        PlanStepInfo {
                            id: st.id.clone(),
                            peer_id: tool
                                .map(|t| t.peer_id.clone())
                                .unwrap_or_else(|| st.peer.clone()),
                            peer_short: st.peer.clone(),
                            capability: st.capability.clone(),
                        }
                    })
                    .collect();
                let _ = tx.send(DispatchEvent::PlanReady {
                    steps,
                    round: round + 1,
                });
            }

            // One journal row per compiled plan, as the table documents.
            let round_run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
            let journalled = serde_json::to_string(&next).ok().and_then(|plan_json| {
                n3ur0n_storage::plan_runs::insert(
                    node.db(),
                    &n3ur0n_storage::plan_runs::PlanRunRecord {
                        id: round_run_id.clone(),
                        conversation_id: state.id.clone(),
                        plan_json,
                        status: "running".into(),
                        created_at: round_started_at,
                        finished_at: None,
                    },
                )
                .ok()
            });
            if journalled.is_none() {
                // Round 1 refuses to execute an unjournalled plan; round 2 used
                // to execute one silently, so a crash could leave work with no
                // trace of the plan that caused it.
                warn!(
                    round,
                    "could not journal the continuation plan; not executing it"
                );
                break;
            }

            // Tool turns of this round continue where the previous one stopped.
            // `persist_tool_pair_at` derives its seq from `base_seq + 2*index`,
            // so the offset belongs on the index, not on the base.
            let index_offset = run.trace.len();
            let db_hook2 = node.db().clone();
            let conv_id_hook2 = state.id.clone();
            let fallback2 = node.instance_id();
            let mut on_done2 = |idx: usize, entry: &TraceEntry| {
                let peer = n3ur0n_core::InstanceId::parse(&entry.peer_id)
                    .unwrap_or_else(|_| fallback2.clone());
                if let Err(e) = persist_tool_pair_at(
                    &db_hook2,
                    &conv_id_hook2,
                    base_seq,
                    idx + index_offset,
                    &entry.call_id,
                    &peer,
                    &entry.capability,
                    &entry.args,
                    &entry.result,
                    &entry.error,
                    round_started_at,
                ) {
                    warn!(error = %e, step = idx, round, "failed to persist continuation tool turn");
                }
            };

            let next_run = match execute_plan_streaming(
                node,
                &next,
                &catalog,
                events,
                Some(&mut on_done2),
                &blob_owner,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, round, "continuation execution failed; keeping earlier rounds");
                    break;
                }
            };
            let finished = OffsetDateTime::now_utc().unix_timestamp();
            let _ =
                n3ur0n_storage::plan_runs::set_status(node.db(), &round_run_id, "done", finished);

            for entry in &next_run.trace {
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

            // Merge into the accumulated run so reflect sees every round at
            // once: the user asked one question and gets one answer.
            //
            // Step ids are unique within a plan, not across rounds, so a plain
            // `extend` lets round 2's `s1` silently replace round 1's. That was
            // harmless while only the trace was read, and stopped being harmless
            // the moment `referenceable_values` started handing `${s1.field}`
            // tokens to the composer: a quoted reference would resolve to the
            // wrong round's value. Colliding ids are suffixed instead.
            depth = plan_depth(&next);
            for (id, value) in next_run.blackboard {
                let key = if run.blackboard.contains_key(&id) {
                    // No dot: `lookup_path` splits the head on '.', so the
                    // suffix has to stay inside the id segment.
                    format!("{id}_r{}", round + 1)
                } else {
                    id
                };
                run.blackboard.insert(key, value);
            }
            run.trace.extend(next_run.trace);
            if next_run.last_step_id.is_some() {
                run.last_step_id = next_run.last_step_id;
            }
            round += 1;
        }

        // 6. Reflect.
        let outcome = self
            .reflect_only(
                node,
                state,
                &planner_text,
                Some(&run.blackboard_summary()),
                Some(&run.blackboard),
                &resolved_scope.unresolved,
                run.trace,
                rounds_used,
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
    #[allow(clippy::too_many_arguments)] // every argument is a distinct input the
    // reply depends on; bundling them into a struct would only move the list.
    async fn reflect_only(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: &str,
        blackboard_summary: Option<&str>,
        blackboard: Option<&HashMap<String, Value>>,
        unresolved: &[String],
        trace: Vec<TraceEntry>,
        rounds: usize,
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
            let refs = blackboard.map(referenceable_values).unwrap_or_default();
            let quoting = if refs.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\nTo state any of these values, write its reference and nothing \
                     else — the system replaces it with the exact value:\n{refs}\n\
                     Never retype or recompute a value yourself. A reversed string, a \
                     hash, an id or a timestamp retyped from memory will be wrong."
                )
            };
            messages.push(json!({
                "role": "system",
                "content": format!("Blackboard from this dispatch:\n{summary}{quoting}")
            }));
        }
        // A mention that matched nothing is the user's own words coming back
        // unanswered. Saying so is the whole point of not having silently
        // ignored it or emptied the catalogue over it.
        if !unresolved.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": format!(
                    "These mentions in the user's message matched nothing known to this \
                     node: {}. Say so plainly, in their language, before answering with \
                     whatever you could do. Do not pretend they exist, and do not \
                     silently substitute something else for them.",
                    unresolved.join(", ")
                )
            }));
        }
        // A message made only of mentions scopes the catalogue without asking
        // anything. Left unsaid, the model fills the blank — it will happily
        // report having opened a file from an earlier turn.
        if crate::mention::is_scope_only(user_message) {
            messages.push(json!({
                "role": "system",
                "content": "The user's message contains only scoping mentions and no \
            request. Do not guess what they want and do not claim anything was done. Ask them \
            what they would like, in their language."
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
        let raw = response
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        // Substitute any `${step.field}` the composer wrote. Unresolvable ones
        // are left verbatim by `resolve_value`, so a stray reference degrades
        // to visible text rather than to a wrong value.
        let content = match blackboard {
            Some(bb) => crate::planner::plan::resolve_value(&Value::String(raw), bb)
                .as_str()
                .map(str::to_string)
                .unwrap_or_default(),
            None => raw,
        };
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
            rounds,
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
/// The blackboard rendered as *references the composer can quote*.
///
/// The narrative summary names capabilities but no step ids, so a model asked
/// to state a tool's output has no handle for it and can only retype the value.
/// Measured on a three-step chain, a 7B retyped `reverse`'s output by trying to
/// redo the reversal in its head and produced a string that was not the one the
/// tool returned — the character count beside it, being a short number, was
/// copied correctly.
///
/// Listing `${step.field}` tokens gives it something to write instead of a
/// value, and [`resolve_value`] substitutes the real one afterwards. Fabrication
/// stops being discouraged and becomes impossible for anything quoted this way.
fn referenceable_values(blackboard: &HashMap<String, Value>) -> String {
    let mut ids: Vec<&String> = blackboard.keys().collect();
    ids.sort();
    let mut out = String::new();
    for id in ids {
        match blackboard.get(id) {
            Some(Value::Object(fields)) => {
                for (field, value) in fields {
                    let _ = writeln!(
                        out,
                        "  ${{{id}.{field}}} = {}",
                        render_blackboard_value(value)
                    );
                }
            }
            Some(other) => {
                let _ = writeln!(out, "  ${{{id}}} = {}", render_blackboard_value(other));
            }
            None => {}
        }
    }
    out
}

/// Identity of one tool invocation: who, what, and with exactly which values.
///
/// `serde_json` keeps object keys sorted, so the rendered args are canonical
/// and `{"a":1,"b":2}` compares equal to `{"b":2,"a":1}`.
fn invocation_fingerprint(peer_id: &str, capability: &str, args: &Value) -> String {
    format!(
        "{peer_id}::{capability}::{}",
        serde_json::to_string(args).unwrap_or_default()
    )
}

/// What already ran, by full peer id — the trace carries **resolved** args, so
/// these are the values actually sent.
fn executed_invocations(trace: &[TraceEntry]) -> BTreeSet<String> {
    trace
        .iter()
        .map(|e| invocation_fingerprint(&e.peer_id, &e.capability, &e.args))
        .collect()
}

/// True when a continuation would only redo invocations that already ran.
///
/// The first version of this guard compared whole plans for equality and let a
/// subset through, so `reverse` and `string_length` ran twice on a three-step
/// chain. The second compared `peer::capability` and ignored arguments
/// entirely, which fixed that but blocked "summarise document A, then document
/// B" — a real request, refused.
///
/// Comparing arguments works because the two sides are directly comparable: the
/// trace holds args *resolved* at execution time, and a continuation writes
/// literals copied out of the blackboard. (The initial plan's `${refs}` never
/// enter the comparison; that was the mistaken reason for avoiding args.)
///
/// A step whose args still contain a reference is treated as new: it points at
/// something this round has not produced yet, so its value is unknown and
/// cannot match anything already executed.
fn adds_nothing(next: &Plan, trace: &[TraceEntry], catalog: &Catalog) -> bool {
    if trace.is_empty() {
        return false;
    }
    let already = executed_invocations(trace);
    next.plan.iter().all(|step| {
        let mut refs = std::collections::HashSet::new();
        crate::planner::plan::collect_refs(&step.args, &mut refs);
        if !refs.is_empty() {
            return false; // depends on a value this round will produce
        }
        let Some(tool) = catalog.find(&format!("{}::{}", step.peer, step.capability)) else {
            return false; // unknown tool: let validation speak, not this guard
        };
        already.contains(&invocation_fingerprint(
            &tool.peer_id,
            &step.capability,
            &step.args,
        ))
    })
}

/// Why a dispatch should plan another round, or `None` to stop.
///
/// Both signals are observations the runtime makes on its own. Neither asks the
/// model whether it is finished — that self-assessment is what a small model is
/// worst at, and its dominant failure (over-planning) would bias it towards
/// answering "not yet" every time.
fn continuation_reason(
    trace: &[TraceEntry],
    depth: usize,
    elapsed_secs: i64,
) -> Option<&'static str> {
    // Budgets first: a reason to continue is worth nothing if there is no room
    // left to do it in.
    if trace.len() >= MAX_TOTAL_STEPS || elapsed_secs >= DISPATCH_BUDGET_SECS {
        return None;
    }
    if trace.iter().any(|e| e.error.is_some()) {
        // A failed step means the plan did not do what it set out to; there is
        // something left to attempt or to report.
        return Some("a step failed");
    }
    if depth >= CONTINUATION_DEPTH_THRESHOLD {
        // Deep enough that the plan may have stopped short of the goal rather
        // than at it.
        return Some("plan is deep enough to suspect it stopped short");
    }
    None
}

/// Prompt for a continuation round.
///
/// The model is not asked whether the work is finished — a self-assessment a
/// small model is poor at. It is asked the same question as before, with what
/// already ran in front of it, and `{"plan": []}` is the ordinary way to say
/// there is nothing left to do.
fn continuation_message(user_msg: &str, blackboard: &str) -> String {
    format!(
        "{user_msg}\n\n\
         [steps already executed] These ran and produced:\n\
         {blackboard}\n\
         \n\
         Plan ONLY what still has to happen, given those results. Do not repeat \
         a step that already ran, and do not re-plan work whose result is above. \
         A step that reports an error has already been attempted and will fail \
         the same way again: do not retry it. Either plan a different route to \
         the same goal, or stop. \
         Every capability listed above has already run; calling it again \
         recomputes a value you already have. \
         If the results are enough to answer the user, return {{\"plan\": []}} — \
         that is the normal ending, not a failure."
    )
}

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

/// What an `@` mention could be matched to, and what it could not.
///
/// A mention states an intention; it does not assert the thing exists. So an
/// entity that matches nothing must neither narrow the catalogue to nothing nor
/// vanish in silence — the first refuses to help over a typo, the second lets
/// the user believe they scoped something when they did not. Both decide on
/// their behalf without telling them.
///
/// Only what resolved narrows the catalogue. What did not is carried to the
/// reply, so the answer can say plainly that there is no such peer, lobe or
/// capability. The unresolved name still reaches retrieval through the message
/// text, which is where it belongs: a hint, not a filter.
#[derive(Debug, Default)]
struct ResolvedScope {
    peer_ids: Vec<String>,
    lobes: Vec<String>,
    capabilities: Vec<String>,
    /// Mentions that matched nothing, rendered as the user wrote them.
    unresolved: Vec<String>,
}

impl ResolvedScope {
    fn narrows(&self) -> bool {
        !self.peer_ids.is_empty() || !self.lobes.is_empty() || !self.capabilities.is_empty()
    }
}

/// Match a parsed scope against the peer directory and the catalogue.
///
/// Peers resolve by canonical `n3:` id or by the short form the UI shows —
/// never by self-declared alias, which is a claim a peer makes about itself and
/// would hand every mention of a name to its first squatter.
fn resolve_scope(node: &Node, catalog: &Catalog, scope: &MentionScope) -> ResolvedScope {
    let mut out = ResolvedScope::default();

    if !scope.peers.is_empty() {
        let self_id = node.instance_id().to_string();
        for entity in &scope.peers {
            // `@peer:self` is the one name that cannot be squatted: it never
            // travels, and it means "the instance I am talking to" whoever
            // that is. Everything else has to be an id precisely because a
            // name a peer gives itself proves nothing.
            if entity.eq_ignore_ascii_case("self") {
                if !out.peer_ids.contains(&self_id) {
                    out.peer_ids.push(self_id.clone());
                }
                continue;
            }
            // `alias#shortid` — the readable half is decoration, the id binds.
            // Writing `@peer:n3:ynhr3l57…` in a sentence is unreadable, and
            // writing `@peer:toolbox` alone would route by a name a peer
            // asserts about itself. Carrying both keeps the sentence legible
            // without ever trusting the label: only the part after `#` is
            // matched, so a stale or forged alias changes nothing.
            let key = entity.rsplit('#').next().unwrap_or(entity.as_str());

            // The instance itself is not in its own directory, so it is
            // matched here rather than looked up.
            let self_matches = self_id == key
                || self_id
                    .strip_prefix("n3:")
                    .is_some_and(|short| key.len() >= 8 && short.starts_with(key));

            // A prefix lookup on the primary key, not a scan of the whole
            // directory: the label half of the token is never a search term,
            // so there is nothing to match but the id.
            let hit = if self_matches {
                Some(self_id.clone())
            } else if key.len() >= 8 || key.starts_with("n3:") {
                match n3ur0n_storage::peers::find_by_id_prefix(node.db(), key, 2) {
                    Ok(found) if found.len() == 1 => Some(found[0].id.clone()),
                    // Two peers behind one prefix: say so instead of picking.
                    Ok(found) if found.len() > 1 => {
                        warn!(prefix = %key, "peer mention is ambiguous");
                        None
                    }
                    Ok(_) => None,
                    Err(e) => {
                        warn!(error = %e, "peer directory unavailable; peer mentions cannot resolve");
                        None
                    }
                }
            } else {
                None
            };
            match hit {
                Some(id) if !out.peer_ids.contains(&id) => out.peer_ids.push(id),
                Some(_) => {}
                None => out.unresolved.push(format!("@peer:{entity}")),
            }
        }
    }

    for lobe in &scope.lobes {
        if catalog
            .tools
            .iter()
            .any(|t| t.cap.lobe_ids.iter().any(|l| l == lobe))
        {
            out.lobes.push(lobe.clone());
        } else {
            out.unresolved.push(format!("@lobe:{lobe}"));
        }
    }

    for cap in &scope.capabilities {
        if catalog.tools.iter().any(|t| &t.cap.name == cap) {
            out.capabilities.push(cap.clone());
        } else {
            out.unresolved.push(format!("@cap:{cap}"));
        }
    }

    out
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

    fn failed_entry() -> TraceEntry {
        TraceEntry {
            error: Some("peer unreachable".into()),
            result: None,
            ..trace_entry("cap", json!({}))
        }
    }

    #[test]
    fn a_clean_shallow_run_does_not_continue() {
        assert!(continuation_reason(&[trace_entry("cap", json!({}))], 1, 0).is_none());
        assert!(continuation_reason(&[], 0, 0).is_none());
    }

    #[test]
    fn a_failed_step_continues() {
        assert_eq!(
            continuation_reason(&[trace_entry("cap", json!({})), failed_entry()], 1, 0),
            Some("a step failed")
        );
    }

    #[test]
    fn a_deep_plan_continues() {
        assert!(
            continuation_reason(
                &[trace_entry("cap", json!({}))],
                CONTINUATION_DEPTH_THRESHOLD,
                0
            )
            .is_some()
        );
        // One below the threshold is a plan that stopped on its own.
        assert!(
            continuation_reason(
                &[trace_entry("cap", json!({}))],
                CONTINUATION_DEPTH_THRESHOLD - 1,
                0
            )
            .is_none()
        );
    }

    #[test]
    fn the_step_budget_outranks_every_reason_to_continue() {
        let spent: Vec<TraceEntry> = (0..MAX_TOTAL_STEPS).map(|_| failed_entry()).collect();
        assert!(
            continuation_reason(&spent, CONTINUATION_DEPTH_THRESHOLD, 0).is_none(),
            "a failed step and a deep plan must not buy a round there is no budget for"
        );
    }

    #[test]
    fn the_time_budget_outranks_every_reason_to_continue() {
        assert!(
            continuation_reason(
                &[failed_entry()],
                CONTINUATION_DEPTH_THRESHOLD,
                DISPATCH_BUDGET_SECS
            )
            .is_none()
        );
    }

    fn ran(cap: &str, args: Value) -> TraceEntry {
        TraceEntry {
            peer_id: "n3:p1".into(),
            args,
            ..trace_entry(cap, json!({}))
        }
    }

    /// Plans carry the short peer form, the trace carries full ids; the
    /// catalogue is what translates between them.
    fn catalog_with(peer_id: &str) -> Catalog {
        Catalog {
            tools: vec![
                tool_def(peer_id, "reverse"),
                tool_def(peer_id, "string_length"),
                tool_def(peer_id, "summarize"),
                tool_def(peer_id, "translate"),
            ],
        }
    }

    fn tool_def(peer_id: &str, name: &str) -> ToolDef {
        ToolDef {
            peer_id: peer_id.into(),
            peer_endpoint: None,
            cap: n3ur0n_core::capability::CapabilityDecl {
                name: name.into(),
                description: String::new(),
                schema_in: json!({}),
                schema_out: json!({}),
                mode: n3ur0n_core::capability::AccessMode::Free,
                pricing: None,
                tags: vec![],
                lobe_ids: vec![],
                examples: vec![],
                disambiguation: None,
                negative_examples: vec![],
                output_semantic: None,
                version: "0.0.0".into(),
                languages: vec![],
                countries: vec![],
            },
        }
    }

    fn short_peer_of(cat: &Catalog) -> String {
        cat.tool_name(&cat.tools[0])
            .split_once("::")
            .map(|(p, _)| p.to_string())
            .unwrap()
    }

    fn step_on(peer: &str, cap: &str, args: Value) -> crate::planner::plan::PlanStep {
        crate::planner::plan::PlanStep {
            id: format!("s_{cap}"),
            peer: peer.into(),
            capability: cap.into(),
            args,
            depends_on: vec![],
        }
    }

    #[test]
    fn a_continuation_repeating_every_invocation_adds_nothing() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        let next = Plan {
            plan: vec![
                step_on(&p, "reverse", json!({"text": "abc"})),
                step_on(&p, "string_length", json!({"text": "cba"})),
            ],
        };
        let trace = vec![
            ran("reverse", json!({"text": "abc"})),
            ran("string_length", json!({"text": "cba"})),
        ];
        assert!(adds_nothing(&next, &trace, &cat));
    }

    /// The case the previous `peer::capability` guard wrongly refused.
    #[test]
    fn the_same_capability_on_new_data_is_allowed() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        let next = Plan {
            plan: vec![step_on(&p, "summarize", json!({"text": "document B"}))],
        };
        let trace = vec![ran("summarize", json!({"text": "document A"}))];
        assert!(!adds_nothing(&next, &trace, &cat));
    }

    #[test]
    fn argument_key_order_does_not_matter() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        let next = Plan {
            plan: vec![step_on(&p, "translate", json!({"to": "fr", "text": "hi"}))],
        };
        let trace = vec![ran("translate", json!({"text": "hi", "to": "fr"}))];
        assert!(adds_nothing(&next, &trace, &cat));
    }

    #[test]
    fn a_step_still_holding_a_reference_counts_as_new() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        // Its value is unknown until this round runs, so it cannot be a repeat.
        let next = Plan {
            plan: vec![step_on(&p, "reverse", json!({"text": "${s1.now}"}))],
        };
        let trace = vec![ran("reverse", json!({"text": "abc"}))];
        assert!(!adds_nothing(&next, &trace, &cat));
    }

    #[test]
    fn one_new_invocation_among_repeats_lets_the_round_run() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        let next = Plan {
            plan: vec![
                step_on(&p, "reverse", json!({"text": "abc"})),
                step_on(&p, "translate", json!({"text": "abc", "to": "fr"})),
            ],
        };
        let trace = vec![ran("reverse", json!({"text": "abc"}))];
        assert!(!adds_nothing(&next, &trace, &cat));
    }

    #[test]
    fn nothing_executed_yet_never_blocks() {
        let cat = catalog_with("n3:p1");
        let p = short_peer_of(&cat);
        let next = Plan {
            plan: vec![step_on(&p, "reverse", json!({}))],
        };
        assert!(!adds_nothing(&next, &[], &cat));
    }

    #[test]
    fn referenceable_values_lists_one_token_per_field() {
        let mut bb = HashMap::new();
        bb.insert(
            "s1".to_string(),
            json!({"now": "2026-09-12T18:06:29.780871607Z"}),
        );
        bb.insert(
            "s2".to_string(),
            json!({"reversed": "Z706178087.92:60:81T21-90-6202"}),
        );
        bb.insert("s3".to_string(), json!({"chars": 30, "bytes": 30}));
        let out = referenceable_values(&bb);

        assert!(out.contains("${s1.now}"), "{out}");
        assert!(out.contains("${s2.reversed}"), "{out}");
        assert!(out.contains("${s3.chars}"), "{out}");
        // The value is shown beside the token, so the model sees what it stands
        // for without having to guess.
        assert!(out.contains("Z706178087.92:60:81T21-90-6202"), "{out}");
        // Sorted by step id: the composer reads them in execution order.
        assert!(out.find("${s1.").unwrap() < out.find("${s2.").unwrap());
    }

    #[test]
    fn referenceable_values_handles_a_non_object_result() {
        let mut bb = HashMap::new();
        bb.insert("s1".to_string(), json!(42));
        assert!(referenceable_values(&bb).contains("${s1} = 42"));
    }

    /// The other half of the mechanism: a reference the composer writes into
    /// prose is replaced by the exact value, so what it quotes cannot drift
    /// from what the tool returned.
    #[test]
    fn a_reference_inside_prose_is_substituted_verbatim() {
        let mut bb = HashMap::new();
        bb.insert(
            "s2".to_string(),
            json!({"reversed": "Z706178087.92:60:81T21-90-6202"}),
        );
        bb.insert("s3".to_string(), json!({"chars": 30}));

        let written = Value::String(
            "The reversed string is ${s2.reversed}, which has ${s3.chars} characters.".into(),
        );
        let resolved = crate::planner::plan::resolve_value(&written, &bb);

        assert_eq!(
            resolved.as_str().unwrap(),
            "The reversed string is Z706178087.92:60:81T21-90-6202, which has 30 characters."
        );
    }

    /// A reference to something that does not exist must degrade to visible
    /// text, never to a wrong value silently substituted.
    #[test]
    fn an_unknown_reference_is_left_alone() {
        let bb = HashMap::new();
        let written = Value::String("value: ${s9.nope}".into());
        let resolved = crate::planner::plan::resolve_value(&written, &bb);
        assert_eq!(resolved.as_str().unwrap(), "value: ${s9.nope}");
    }

    /// Round 2 reusing `s1` must not shadow round 1's value, now that the
    /// composer is handed `${s1.field}` tokens to quote.
    #[test]
    fn merging_rounds_keeps_both_values_for_a_reused_step_id() {
        let mut merged: HashMap<String, Value> = HashMap::new();
        merged.insert("s1".into(), json!({"now": "first"}));

        let round_two: Vec<(String, Value)> = vec![
            ("s1".into(), json!({"now": "second"})),
            ("s2".into(), json!({"other": 1})),
        ];
        // Mirrors the merge in `dispatch_inner`.
        for (id, value) in round_two {
            let key = if merged.contains_key(&id) {
                format!("{id}_r2")
            } else {
                id
            };
            merged.insert(key, value);
        }

        assert_eq!(merged["s1"], json!({"now": "first"}));
        assert_eq!(merged["s1_r2"], json!({"now": "second"}));
        assert_eq!(merged["s2"], json!({"other": 1}));

        // And both stay quotable, which is the point of keeping them apart.
        let rendered = referenceable_values(&merged);
        assert!(rendered.contains("${s1.now}"), "{rendered}");
        assert!(rendered.contains("${s1_r2.now}"), "{rendered}");
    }

    fn scope_catalog() -> Catalog {
        let mut summarize = tool_def("n3:p1", "summarize");
        summarize.cap.lobe_ids = vec!["medical".into()];
        Catalog {
            tools: vec![summarize, tool_def("n3:p2", "translate")],
        }
    }

    fn node_for_scope() -> Node {
        Node::new(
            n3ur0n_core::Keypair::generate(),
            n3ur0n_storage::open_in_memory().unwrap(),
            std::sync::Arc::new(n3ur0n_adapters::echo::EchoBackend),
            crate::registry::CapabilityRegistry::from_decls(vec![]),
            crate::node::NodeConfig::default(),
        )
    }

    #[test]
    fn a_known_capability_narrows_the_catalogue() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@cap:summarize"));
        assert_eq!(r.capabilities, vec!["summarize"]);
        assert!(r.unresolved.is_empty());
        assert!(r.narrows());
    }

    /// The correction that matters: naming a capability is not claiming it
    /// exists. An unknown one must not empty the catalogue, and must not be
    /// swallowed either — the user has to learn it was not found.
    #[test]
    fn an_unknown_capability_neither_narrows_nor_disappears() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@cap:transcribe"));
        assert!(r.capabilities.is_empty());
        assert!(
            !r.narrows(),
            "an unknown name must not scope the catalogue to nothing"
        );
        assert_eq!(r.unresolved, vec!["@cap:transcribe"]);
    }

    #[test]
    fn an_unknown_lobe_is_reported_instead_of_emptying_the_catalogue() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@lobe:finance"));
        assert!(!r.narrows());
        assert_eq!(r.unresolved, vec!["@lobe:finance"]);

        // A lobe something actually carries still scopes.
        let known = resolve_scope(&node, &cat, &MentionScope::from_text("@lobe:medical"));
        assert_eq!(known.lobes, vec!["medical"]);
        assert!(known.unresolved.is_empty());
    }

    #[test]
    fn a_peer_mention_can_carry_a_readable_alias_before_the_id() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let id = node.instance_id().to_string();
        let short = &id["n3:".len()..][..8];

        // The id alone still works, and so does the readable compound form.
        let bare = resolve_scope(&node, &cat, &MentionScope::from_text(&format!("@peer:{short}")));
        let named = resolve_scope(
            &node,
            &cat,
            &MentionScope::from_text(&format!("@peer:toolbox#{short}")),
        );
        assert_eq!(bare.peer_ids, vec![id.clone()]);
        assert_eq!(named.peer_ids, vec![id.clone()]);

        // The label is never matched: a wrong one resolves to the same peer,
        // which is the whole point of putting the id in the token.
        let lying = resolve_scope(
            &node,
            &cat,
            &MentionScope::from_text(&format!("@peer:not-my-name#{short}")),
        );
        assert_eq!(lying.peer_ids, vec![id]);

        // And a label with no id behind it resolves to nothing.
        let label_only = resolve_scope(&node, &cat, &MentionScope::from_text("@peer:toolbox"));
        assert!(label_only.peer_ids.is_empty());
        assert_eq!(label_only.unresolved, vec!["@peer:toolbox"]);
    }

    #[test]
    fn peer_self_resolves_to_this_instance() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@peer:self"));
        assert_eq!(r.peer_ids, vec![node.instance_id().to_string()]);
        assert!(r.unresolved.is_empty());
        // Case does not matter, and it is not a name a peer could claim: an
        // instance calling itself "self" still resolves by id, not by label.
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@peer:SELF"));
        assert_eq!(r.peer_ids, vec![node.instance_id().to_string()]);
    }

    #[test]
    fn an_unknown_peer_is_reported_instead_of_being_dropped() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(&node, &cat, &MentionScope::from_text("@peer:nosuchpeer00"));
        assert!(r.peer_ids.is_empty());
        assert_eq!(r.unresolved, vec!["@peer:nosuchpeer00"]);
    }

    #[test]
    fn what_resolves_still_narrows_beside_what_does_not() {
        let node = node_for_scope();
        let cat = scope_catalog();
        let r = resolve_scope(
            &node,
            &cat,
            &MentionScope::from_text("@cap:summarize @cap:transcribe"),
        );
        assert_eq!(r.capabilities, vec!["summarize"]);
        assert_eq!(r.unresolved, vec!["@cap:transcribe"]);
        assert!(r.narrows(), "one bad name must not cancel a good one");
    }
}
