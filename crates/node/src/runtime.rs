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
    planner: Arc<dyn Planner>,
    config: RuntimeConfig,
    planner_slots: Arc<Semaphore>,
    conv_locks: Arc<std::sync::Mutex<std::collections::HashMap<String, ConvMutex>>>,
    cache: Arc<Mutex<LruCache<String, ConversationState>>>,
}

impl std::fmt::Debug for NodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRuntime")
            .field("planner", &self.planner)
            .field("config", &self.config)
            .finish()
    }
}

impl NodeRuntime {
    pub fn new(node: Node, planner: Arc<dyn Planner>, config: RuntimeConfig) -> Self {
        let cap = NonZeroUsize::new(config.max_active_conversations.max(1))
            .expect("max_active_conversations was clamped to >=1");
        Self {
            node,
            planner,
            planner_slots: Arc::new(Semaphore::new(config.max_concurrent_planners.max(1))),
            conv_locks: Arc::new(std::sync::Mutex::new(Default::default())),
            cache: Arc::new(Mutex::new(LruCache::new(cap))),
            config,
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

    async fn load_state(
        &self,
        conv_id: &str,
        client_id: &str,
    ) -> NodeResult<ConversationState> {
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
        let state = conversation::load(self.node.db(), conv_id, client_id)
            .map_err(map_conv_err)?;
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
        message: String,
    ) -> NodeResult<DispatchOutcome> {
        self.handle_user_message_with_opts(client_id, conv_id, message, DispatchMode::default(), DispatchOptions::default())
            .await
    }

    /// Process a user message with explicit dispatch mode and options.
    pub async fn handle_user_message_with_opts(
        &self,
        client_id: &str,
        conv_id: &str,
        message: String,
        mode: DispatchMode,
        opts: DispatchOptions,
    ) -> NodeResult<DispatchOutcome> {
        // Per-conversation serialisation.
        let conv_lock = self.lock_for(conv_id);
        let _guard = conv_lock.lock().await;

        // Global LLM/peer concurrency cap.
        let _slot = self.acquire_planner_slot().await;

        let mut state = self.load_state(conv_id, client_id).await?;
        let outcome = self
            .planner
            .dispatch(&self.node, &mut state, message, mode, opts)
            .await?;
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
        message: String,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        self.handle_user_message_streaming_with_opts(
            client_id,
            conv_id,
            message,
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
        message: String,
        mode: DispatchMode,
        opts: DispatchOptions,
        events: EventSender,
    ) -> NodeResult<DispatchOutcome> {
        let conv_lock = self.lock_for(conv_id);
        let _guard = conv_lock.lock().await;

        let _slot = self.acquire_planner_slot().await;

        let mut state = self.load_state(conv_id, client_id).await?;
        let outcome = self
            .planner
            .dispatch_streaming(&self.node, &mut state, message, mode, opts, events)
            .await?;
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
