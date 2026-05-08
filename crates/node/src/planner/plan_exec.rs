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
use crate::planner::catalog::Catalog;
use crate::planner::plan::{execute_plan, validate_plan, Plan};
use crate::planner::{DispatchOutcome, Planner, TraceEntry};

const MAX_CONTEXT_TURNS: usize = 16;

#[derive(Clone)]
pub struct PlanExecPlanner {
    pub llm_backend: Arc<dyn Backend>,
    pub model_hint: Option<String>,
}

impl std::fmt::Debug for PlanExecPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanExecPlanner")
            .field("model_hint", &self.model_hint)
            .finish()
    }
}

impl PlanExecPlanner {
    pub fn new(llm_backend: Arc<dyn Backend>, model_hint: Option<String>) -> Self {
        Self { llm_backend, model_hint }
    }

    fn compile_system_prompt(&self, catalog: &Catalog) -> String {
        let mut s = String::from(
            "You are an n3ur0n plan compiler. Given a user request, produce ONE JSON \
plan that a deterministic executor will run. Output ONLY valid JSON conforming to \
this schema:\n\n\
{\n\
  \"plan\": [\n\
    {\n\
      \"id\":         \"<short alpha-numeric id, unique>\",\n\
      \"peer\":       \"<short_peer from the tools list>\",\n\
      \"capability\": \"<capability name from the tools list>\",\n\
      \"args\":       { ...capability-specific args... },\n\
      \"depends_on\": [ \"<other step ids>\", ... ]\n\
    },\n\
    ...\n\
  ]\n\
}\n\n\
Rules:\n\
- Return ONLY the JSON; no prose, no Markdown fences.\n\
- Reference results from earlier steps inside `args` with the exact syntax \
`${stepid.path.to.value}` — the dollar sign is required. Example:\n\
    {\"id\": \"s1\", \"peer\": \"abc\", \"capability\": \"random_int\", \"args\": {\"min\":1,\"max\":10}}\n\
    {\"id\": \"s2\", \"peer\": \"abc\", \"capability\": \"reverse\", \"args\": {\"text\": \"${s1.value}\"}}\n\
    {\"id\": \"s3\", \"peer\": \"xyz\", \"capability\": \"chat\",\n\
     \"args\": {\"prompt\": \"Write one rhyming line about ${s2.reversed}\"}}\n\
- A reference inside args creates an implicit dependency (omit from `depends_on`).\n\
- For chat-like tools, set only fields the schema declares; never set `model`.\n\
- Pick the SHORTEST useful plan. If the user asks something you can answer from \
prior knowledge with no tool, return an empty plan: `{\"plan\": []}`.\n\
- All `peer` values must come from the tools list verbatim.\n\
\n\
Available tools (peer::capability — description, schema_in):\n",
        );
        if catalog.is_empty() {
            s.push_str("(none)\n");
        } else {
            for t in &catalog.tools {
                let name = catalog.tool_name(t);
                s.push_str(&format!(
                    "- {} — {} (input schema: {})\n",
                    name,
                    t.cap.description,
                    serde_json::to_string(&t.cap.schema_in).unwrap_or_else(|_| "?".into())
                ));
            }
        }
        s
    }

    fn reflect_system_prompt(&self) -> String {
        String::from(
            "You are an n3ur0n response composer. The user asked a question and a plan \
was executed; you now have the executor's blackboard (all step results). Write the \
final reply for the user in plain text. Use the user's language. Do NOT emit JSON, \
do NOT call any tool, do NOT re-explain the plan unless the user asked for it. Be \
concise and use the actual values from the blackboard.",
        )
    }
}

#[async_trait]
impl Planner for PlanExecPlanner {
    async fn dispatch(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
    ) -> NodeResult<DispatchOutcome> {
        // 1. Persist user turn.
        state.push_user(user_message.clone());
        persist_last(node.db(), state)
            .map_err(|e| NodeError::InvalidPayload(format!("persist user: {e}")))?;

        // 2. Build catalog.
        let catalog = Catalog::build(
            node.instance_id().as_str(),
            node.registry(),
            node.db(),
            500,
        )?;

        // 3. Compile: ask the LLM for a Plan. JSON-format mode.
        let compile_messages = vec![
            json!({"role": "system", "content": self.compile_system_prompt(&catalog)}),
            json!({"role": "user", "content": user_message.clone()}),
        ];
        let mut compile_args = json!({
            "messages": compile_messages,
            // Force JSON output (Ollama / llama.cpp interpret this; harmless on
            // upstreams that don't).
            "format": "json",
            "temperature": 0.0,
        });
        if let Some(model) = &self.model_hint {
            compile_args["model"] = Value::String(model.clone());
        }
        let compile_resp = self.llm_backend.invoke("chat", compile_args).await?;
        let raw_plan_json = compile_resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let plan = match parse_plan(&raw_plan_json) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, raw = %raw_plan_json.chars().take(200).collect::<String>(),
                    "plan compile produced invalid JSON; falling back to direct reply");
                // Fall back: reflect on the raw user message alone.
                return self
                    .reflect_only(node, state, &user_message, None, Vec::new())
                    .await;
            }
        };

        // 4. Validate.
        if let Err(e) = validate_plan(&plan, &catalog) {
            warn!(error = %e, "plan validation failed; falling back to direct reply");
            return self
                .reflect_only(node, state, &user_message, None, Vec::new())
                .await;
        }

        // Special case: empty plan = answer directly without any tool.
        if plan.plan.is_empty() {
            return self
                .reflect_only(node, state, &user_message, None, Vec::new())
                .await;
        }

        // 5. Execute.
        let run = execute_plan(node, &plan, &catalog).await?;

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
            &user_message,
            Some(&run.blackboard_summary()),
            run.trace,
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
    ) -> NodeResult<DispatchOutcome> {
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

fn short(peer_id: &str) -> String {
    let trimmed = peer_id.strip_prefix("n3:").unwrap_or(peer_id);
    trimmed.chars().take(12).collect()
}

/// Parse a JSON plan with two fallbacks:
/// 1. Direct serde parse (LLM emits exactly the schema).
/// 2. Look for the first `{` ... matching `}` and try parsing that.
fn parse_plan(raw: &str) -> Result<Plan, String> {
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
}
