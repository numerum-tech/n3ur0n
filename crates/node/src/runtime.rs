//! Runtime orchestration for multi-conversation, multi-client workloads.
//!
//! `NodeRuntime` wraps a [`Node`](crate::node::Node) plus a
//! [`Planner`](crate::planner::Planner), and adds:
//! - mutex per `conversation_id` (serialise dispatches on the same thread)
//! - global semaphore for concurrent planner dispatches (LLM backpressure)
//! - LRU cache of hot `ConversationState` objects
//!
//! HTTP handlers call `runtime.handle_user_message(...)`; the runtime takes
//! care of locking, loading state from SQLite if necessary, running the
//! planner, persisting turns, and returning the final reply.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::conversation::{self, ConversationError, ConversationState};
use crate::error::{NodeError, NodeResult};
use crate::node::Node;
use crate::planner::{DispatchMode, DispatchOptions, DispatchOutcome, EventSender, Planner};

/// Configurable bounds for the runtime.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub max_concurrent_planners: usize,
    pub max_active_conversations: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_concurrent_planners: 4,
            max_active_conversations: 50,
        }
    }
}

/// Per-conversation lock entry held in the runtime's `conv_locks` map.
type ConvMutex = Arc<Mutex<()>>;

#[derive(Clone)]
pub struct NodeRuntime {
    node: Node,
    /// Plan-then-execute planner (`DispatchMode::Auto`).
    planner: Arc<dyn Planner>,
    /// Single-LLM-call planner (`DispatchMode::Direct`).
    direct: Arc<dyn Planner>,
    config: RuntimeConfig,
    planner_slots: Arc<Semaphore>,
    conv_locks: Arc<std::sync::Mutex<std::collections::HashMap<String, ConvMutex>>>,
    cache: Arc<Mutex<LruCache<String, ConversationState>>>,
}

impl std::fmt::Debug for NodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRuntime")
            .field("planner", &self.planner)
            .field("direct", &self.direct)
            .field("config", &self.config)
            .finish()
    }
}

impl NodeRuntime {
    pub fn new(
        node: Node,
        planner: Arc<dyn Planner>,
        direct: Arc<dyn Planner>,
        config: RuntimeConfig,
    ) -> Self {
        let cap = NonZeroUsize::new(config.max_active_conversations.max(1))
            .expect("max_active_conversations was clamped to >=1");
        Self {
            node,
            planner,
            direct,
            planner_slots: Arc::new(Semaphore::new(config.max_concurrent_planners.max(1))),
            conv_locks: Arc::new(std::sync::Mutex::new(Default::default())),
            cache: Arc::new(Mutex::new(LruCache::new(cap))),
            config,
        }
    }

    fn planner_for(&self, mode: DispatchMode) -> &Arc<dyn Planner> {
        match mode {
            DispatchMode::Auto => &self.planner,
            DispatchMode::Direct => &self.direct,
        }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    fn lock_for(&self, conv_id: &str) -> ConvMutex {
        let mut map = self.conv_locks.lock().expect("mutex not poisoned");
        map.entry(conv_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn load_state(&self, conv_id: &str, client_id: &str) -> NodeResult<ConversationState> {
        // Check cache.
        {
            let mut cache = self.cache.lock().await;
            if let Some(state) = cache.get(conv_id) {
                if state.client_id != client_id {
                    return Err(NodeError::InvalidPayload(
                        "ownership mismatch (cache)".into(),
                    ));
                }
                return Ok(state.clone());
            }
        }
        // Load from DB.
        let state = conversation::load(self.node.db(), conv_id, client_id).map_err(map_conv_err)?;
        // Insert in cache.
        let mut cache = self.cache.lock().await;
        cache.put(conv_id.to_string(), state.clone());
        Ok(state)
    }

    async fn store_in_cache(&self, state: ConversationState) {
        let mut cache = self.cache.lock().await;
        cache.put(state.id.clone(), state);
    }

    /// Acquire a semaphore permit, fails fast (does not block forever): the
    /// caller can decide what to do with the await.
    async fn acquire_planner_slot(&self) -> OwnedSemaphorePermit {
        let sem = self.planner_slots.clone();
        sem.acquire_owned()
            .await
            .expect("planner_slots semaphore never closed in v0.1")
    }

    /// Process a user message inside a given conversation:
    ///   load state → mutate via planner → persist → return reply.
    pub async fn handle_user_message(
        &self,
        client_id: &str,
        conv_id: &str,
        input: crate::conversation::UserInput,
    ) -> NodeResult<DispatchOutcome> {
        self.handle_user_message_with_opts(
            client_id,
            conv_id,
            input,
            DispatchMode::default(),
            DispatchOptions::default(),
        )
        .await
    }

    /// Process a user message with explicit dispatch mode and options.
    pub async fn handle_user_message_with_opts(
        &self,
        client_id: &str,
        conv_id: &str,
        input: crate::conversation::UserInput,
        mode: DispatchMode,
        opts: DispatchOptions,
    ) -> NodeResult<DispatchOutcome> {
        // Per-conversation serialisation.
        let conv_lock = self.lock_for(conv_id);
        let _guard = conv_lock.lock().await;

        // Global LLM/peer concurrency cap.
        let _slot = self.acquire_planner_slot().await;

        let mut state = self.load_state(conv_id, client_id).await?;
        let outcome = match self
            .planner_for(mode)
            .dispatch(&self.node, &mut state, input, mode, opts)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                // User turns may already be persisted when the LLM/backend fails.
                // Drop stale cache entries so the next dispatch reloads `next_seq`
                // from SQLite instead of re-inserting the same seq.
                self.evict(conv_id).await;
                return Err(e);
            }
        };
        self.store_in_cache(state).await;
        Ok(outcome)
    }

    /// Same as `handle_user_message` but emits `DispatchEvent`s on the
    /// provided sender as the planner progresses. Caller owns the receiver
    /// half (typically streamed back to the HTTP client as SSE).
    pub async fn handle_user_message_streaming(
        &self,
        client_id: &str,
        conv_id: &str,
        input: crate::conversation::UserInput,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        self.handle_user_message_streaming_with_opts(
            client_id,
            conv_id,
            input,
            DispatchMode::default(),
            DispatchOptions::default(),
            events,
        )
        .await
    }

    /// Same as `handle_user_message_streaming` but with explicit dispatch mode and options.
    pub async fn handle_user_message_streaming_with_opts(
        &self,
        client_id: &str,
        conv_id: &str,
        input: crate::conversation::UserInput,
        mode: DispatchMode,
        opts: DispatchOptions,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        let conv_lock = self.lock_for(conv_id);
        let _guard = conv_lock.lock().await;

        let _slot = self.acquire_planner_slot().await;

        let mut state = self.load_state(conv_id, client_id).await?;
        let outcome = match self
            .planner_for(mode)
            .dispatch_streaming(&self.node, &mut state, input, mode, opts, events)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                self.evict(conv_id).await;
                return Err(e);
            }
        };
        self.store_in_cache(state).await;
        Ok(outcome)
    }

    /// Drop a conversation from cache so the next access re-loads from disk.
    pub async fn evict(&self, conv_id: &str) {
        let mut cache = self.cache.lock().await;
        cache.pop(conv_id);
    }
}

fn map_conv_err(e: ConversationError) -> NodeError {
    match e {
        ConversationError::Storage(s) => NodeError::Storage(s),
        ConversationError::Serde(s) => NodeError::Serde(s),
        ConversationError::NotFound(id) => {
            NodeError::InvalidPayload(format!("conversation not found: {id}"))
        }
        ConversationError::OwnershipMismatch => {
            NodeError::InvalidPayload("ownership mismatch".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeConfig;
    use crate::planner::{DirectChatPlanner, Planner};
    use async_trait::async_trait;
    use n3ur0n_adapters::{AdapterError, AdapterResult, Backend, HealthStatus, echo::EchoBackend};
    use n3ur0n_core::Keypair;
    use n3ur0n_core::capability::CapabilityDecl;
    use n3ur0n_storage::open_in_memory;
    use serde_json::Value;

    struct FailBackend;

    #[async_trait]
    impl Backend for FailBackend {
        async fn invoke(&self, _: &str, _: Value) -> AdapterResult<Value> {
            Err(AdapterError::Backend("upstream down".into()))
        }

        async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
            Ok(vec![])
        }

        async fn health(&self) -> AdapterResult<HealthStatus> {
            Ok(HealthStatus::Healthy)
        }
    }

    /// Reproduces: backend fails after user turn is persisted; stale LRU cache
    /// caused the next send to re-use the same `seq` (UNIQUE constraint).
    #[tokio::test]
    async fn evict_on_failure_avoids_duplicate_seq() {
        let kp = Keypair::generate();
        let db = open_in_memory().unwrap();
        let backend: Arc<dyn Backend> = Arc::new(EchoBackend);
        let registry = crate::registry::CapabilityRegistry::from_decls(vec![]);
        let node = Node::new(kp, db, backend, registry, NodeConfig::default());
        let llm: Arc<dyn Backend> = Arc::new(FailBackend);
        let direct = Arc::new(DirectChatPlanner::new(llm, Some("m".into()))) as Arc<dyn Planner>;
        let auto = direct.clone();
        let runtime = NodeRuntime::new(node.clone(), auto, direct, RuntimeConfig::default());

        let state = conversation::create(node.db(), "client", None).unwrap();
        let conv_id = state.id.clone();

        // Stale cache entry (pre-user-turn snapshot).
        {
            let mut cache = runtime.cache.lock().await;
            cache.put(conv_id.clone(), state);
        }

        let input = conversation::UserInput::from("hello");
        runtime
            .handle_user_message("client", &conv_id, input.clone())
            .await
            .expect_err("first dispatch should fail on LLM");

        let err2 = runtime
            .handle_user_message("client", &conv_id, input)
            .await
            .expect_err("second dispatch should also fail on LLM");
        let msg = err2.to_string();
        assert!(
            !msg.contains("UNIQUE"),
            "stale cache must not cause duplicate seq insert: {msg}"
        );
    }
}
