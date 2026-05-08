//! LLM-driven planner using OpenAI tool-calling natively.
//!
//! Loop: build messages + tools from catalog → call llm_backend → if
//! `tool_calls` present, validate against catalog, signed invoke remote
//! peer, append turn pair, loop. Otherwise final reply, append, return.

use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::Backend;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_core::InstanceId;
use serde_json::{Value, json};

use crate::client as peer_client;
use crate::conversation::{persist_last, ConversationState};
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::catalog::Catalog;
use crate::planner::tool_call::extract_tool_calls;
use crate::planner::{DispatchOutcome, Planner, TraceEntry};

/// Maximum tool-call iterations per user message.
pub const MAX_TOOL_TURNS: usize = 6;

/// Maximum prior turns shown to the LLM (hard cap; longer histories are
/// truncated by `ConversationState::to_chat_messages`).
pub const MAX_CONTEXT_TURNS: usize = 16;

#[derive(Clone)]
pub struct LLMPlanner {
    pub llm_backend: Arc<dyn Backend>,
    pub model_hint: Option<String>,
}

impl std::fmt::Debug for LLMPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LLMPlanner")
            .field("model_hint", &self.model_hint)
            .finish()
    }
}

impl LLMPlanner {
    pub fn new(llm_backend: Arc<dyn Backend>, model_hint: Option<String>) -> Self {
        Self { llm_backend, model_hint }
    }

    fn build_system_prompt(&self, catalog: &Catalog) -> String {
        let mut s = String::from(
            "You are an n3ur0n local planner. The user talks to you in natural \
language. You may call tools (peer capabilities) to fulfil the request. \
Pick at most one tool per turn; or, when the result is sufficient, reply \
to the user directly. Always answer in the user's language.\n\n\
Available tools:\n",
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
}

#[async_trait]
impl Planner for LLMPlanner {
    async fn dispatch(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
    ) -> NodeResult<DispatchOutcome> {
        // 1. Persist the User turn first so a crash mid-dispatch never loses
        //    the user's input.
        state.push_user(user_message);
        persist_last(node.db(), state)
            .map_err(|e| NodeError::InvalidPayload(format!("persist user: {e}")))?;

        let catalog = Catalog::build(
            node.instance_id().as_str(),
            node.registry(),
            node.db(),
            500,
        )?;
        let system_prompt = self.build_system_prompt(&catalog);
        let tools = catalog.to_openai_tools();
        let http_client = peer_client::http_client();

        let mut trace: Vec<TraceEntry> = Vec::new();

        for _iter in 0..MAX_TOOL_TURNS {
            // Build messages: prepend a fresh system prompt each iteration.
            let mut messages: Vec<Value> = Vec::with_capacity(MAX_CONTEXT_TURNS + 1);
            messages.push(json!({"role": "system", "content": system_prompt}));
            messages.extend(state.to_chat_messages(MAX_CONTEXT_TURNS));

            let mut args = json!({
                "messages": messages,
                "tools": tools,
            });
            if let Some(model) = &self.model_hint {
                args["model"] = Value::String(model.clone());
            }
            let response = self.llm_backend.invoke("chat", args).await?;

            let message = response.get("message").cloned().unwrap_or(Value::Null);
            let tool_calls = extract_tool_calls(&message);

            if tool_calls.is_empty() {
                // Final reply.
                let content = message
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let model_used = response
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| self.model_hint.clone());
                state.push_assistant(content.clone(), model_used.clone());
                persist_last(node.db(), state)
                    .map_err(|e| NodeError::InvalidPayload(format!("persist assistant: {e}")))?;
                return Ok(DispatchOutcome {
                    reply: content,
                    model: model_used,
                    trace,
                });
            }

            // Execute each tool call sequentially. The full result of each
            // call goes back in the next LLM iteration via state turns.
            for tc in tool_calls {
                let parsed_args = tc.parsed_args().unwrap_or(Value::Null);
                let resolved = catalog.find(&tc.function.name);

                match resolved {
                    Some(tool) => {
                        let peer_id_full = tool.peer_id.clone();
                        let peer_endpoint = tool.peer_endpoint.clone();
                        let cap_name = tool.cap.name.clone();

                        // Record the planned call.
                        let pid = InstanceId::parse(&peer_id_full).unwrap_or_else(|_| {
                            // Synthetic placeholder if id was somehow malformed
                            // — should not happen since it came from peers DB.
                            node.instance_id()
                        });
                        let call_id = state.push_tool_call(
                            pid.clone(),
                            cap_name.clone(),
                            parsed_args.clone(),
                        );
                        persist_last(node.db(), state).map_err(|e| {
                            NodeError::InvalidPayload(format!("persist tool_call: {e}"))
                        })?;

                        // Local cap = invoke our own backend in-process.
                        // Remote cap = signed envelope.
                        let outcome: Result<Value, String> = if peer_endpoint.is_none() {
                            node.backend()
                                .invoke(&cap_name, parsed_args.clone())
                                .await
                                .map_err(|e| e.to_string())
                        } else {
                            let endpoint = peer_endpoint.clone().unwrap();
                            let payload = json!({
                                "capability": cap_name,
                                "args": parsed_args,
                            });
                            match peer_client::send_signed(
                                &http_client,
                                node.keypair(),
                                &endpoint,
                                ProtocolVerb::Invoke,
                                payload,
                            )
                            .await
                            {
                                Ok(reply) => {
                                    let payload = reply.envelope.payload;
                                    Ok(payload
                                        .get("result")
                                        .cloned()
                                        .unwrap_or(payload))
                                }
                                Err(e) => Err(e.to_string()),
                            }
                        };

                        match outcome {
                            Ok(result) => {
                                state.push_tool_result(
                                    call_id,
                                    pid,
                                    cap_name.clone(),
                                    Some(result.clone()),
                                    None,
                                );
                                trace.push(TraceEntry {
                                    peer_id: peer_id_full,
                                    capability: cap_name,
                                    args: parsed_args,
                                    result: Some(result),
                                    error: None,
                                });
                            }
                            Err(err) => {
                                state.push_tool_result(
                                    call_id,
                                    pid,
                                    cap_name.clone(),
                                    None,
                                    Some(err.clone()),
                                );
                                trace.push(TraceEntry {
                                    peer_id: peer_id_full,
                                    capability: cap_name,
                                    args: parsed_args,
                                    result: None,
                                    error: Some(err),
                                });
                            }
                        }
                        persist_last(node.db(), state).map_err(|e| {
                            NodeError::InvalidPayload(format!("persist tool_result: {e}"))
                        })?;
                    }
                    None => {
                        // Hallucinated tool name. Inject a synthetic
                        // ToolResult error so the LLM can re-plan.
                        let synthetic_id = format!("call_unknown_{}", tc.id);
                        let pid = node.instance_id();
                        state.push(crate::conversation::Turn::ToolCall {
                            id: synthetic_id.clone(),
                            peer_id: pid.clone(),
                            capability: tc.function.name.clone(),
                            args: parsed_args.clone(),
                            ts: time::OffsetDateTime::now_utc().unix_timestamp(),
                        });
                        persist_last(node.db(), state).map_err(|e| {
                            NodeError::InvalidPayload(format!("persist synthetic call: {e}"))
                        })?;
                        let err = format!(
                            "Tool {} is not in this instance's catalog. Available: {}",
                            tc.function.name,
                            catalog
                                .tools
                                .iter()
                                .map(|t| catalog.tool_name(t))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        state.push_tool_result(
                            synthetic_id,
                            pid,
                            tc.function.name.clone(),
                            None,
                            Some(err.clone()),
                        );
                        persist_last(node.db(), state).map_err(|e| {
                            NodeError::InvalidPayload(format!("persist synthetic result: {e}"))
                        })?;
                        trace.push(TraceEntry {
                            peer_id: "<unknown>".into(),
                            capability: tc.function.name,
                            args: parsed_args,
                            result: None,
                            error: Some(err),
                        });
                    }
                }
            }
        }

        // Cap exceeded — append a system turn explaining and return whatever
        // was last said by the assistant (or a generic fallback).
        let fallback = "Maximum tool-call iterations reached without final answer.".to_string();
        state.push_system(format!("planner exceeded {} tool-call iterations", MAX_TOOL_TURNS));
        let _ = persist_last(node.db(), state);
        state.push_assistant(fallback.clone(), self.model_hint.clone());
        let _ = persist_last(node.db(), state);
        Ok(DispatchOutcome {
            reply: fallback,
            model: self.model_hint.clone(),
            trace,
        })
    }
}
