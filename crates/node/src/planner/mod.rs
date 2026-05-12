//! Planner trait + concrete impls.

pub mod catalog;
pub mod plan;
pub mod plan_exec;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::conversation::ConversationState;
use crate::error::NodeResult;
use crate::node::Node;

pub use catalog::{Catalog, ToolDef};
pub use plan_exec::PlanExecPlanner;

/// Outcome of one user message → planner exchange. The state has already
/// been mutated and persisted (one row per turn) by the time this returns.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    /// Final assistant reply (the last `Assistant` turn).
    pub reply: String,
    /// Optional model identifier.
    pub model: Option<String>,
    /// All tool calls executed during this dispatch (for UI trace panel).
    pub trace: Vec<TraceEntry>,
}

#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub peer_id: String,
    pub capability: String,
    pub args: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Channel sender for live dispatch events. Implementations drop it when
/// done.
pub type EventSender = UnboundedSender<DispatchEvent>;

/// One step of a plan as advertised to the UI before execution.
#[derive(Debug, Clone, Serialize)]
pub struct PlanStepInfo {
    pub id: String,
    pub peer_id: String,
    pub peer_short: String,
    pub capability: String,
}

/// Live event stream emitted during a streaming dispatch. Serialised into
/// SSE frames by the HTTP layer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DispatchEvent {
    /// Plan compiled and validated; UI should render the full chip row.
    PlanReady {
        steps: Vec<PlanStepInfo>,
    },
    /// One step starts executing.
    StepStart {
        id: String,
    },
    /// One step finished (with or without error).
    StepDone {
        id: String,
        result: Option<Value>,
        error: Option<String>,
    },
    /// Reflect step is composing the user-facing reply.
    Reflecting,
    /// Final assistant reply ready.
    Final {
        reply: String,
        model: Option<String>,
    },
    /// Fatal error during dispatch; stream is about to close.
    Error {
        message: String,
    },
}

/// Anything that can take a user message + conversation state and produce a
/// reply by talking to peers. Implementations call back into `Node` for
/// signed peer invocations.
#[async_trait]
pub trait Planner: Send + Sync + std::fmt::Debug {
    /// Process a single user message. Mutates `state` (appends User turn,
    /// any number of ToolCall/ToolResult pairs, ends with one Assistant
    /// turn) and returns the dispatch outcome. Persistence of each turn is
    /// the implementation's responsibility — call
    /// `crate::conversation::persist_last(db, state)` after every push.
    async fn dispatch(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
    ) -> NodeResult<DispatchOutcome>;

    /// Streaming variant: same contract as `dispatch`, but emits live
    /// `DispatchEvent`s on the provided channel as the plan runs. Default
    /// impl delegates to non-streaming `dispatch` (no events).
    async fn dispatch_streaming(
        &self,
        node: &Node,
        state: &mut ConversationState,
        user_message: String,
        _events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        self.dispatch(node, state, user_message).await
    }
}
