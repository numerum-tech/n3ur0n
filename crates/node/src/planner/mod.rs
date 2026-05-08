//! Planner trait + concrete impls.

pub mod catalog;
pub mod llm;
pub mod tool_call;

use async_trait::async_trait;

use crate::conversation::ConversationState;
use crate::error::NodeResult;
use crate::node::Node;

pub use catalog::{Catalog, ToolDef};
pub use llm::LLMPlanner;

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
}
