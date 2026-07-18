//! Backend instances: live handles to upstream services.
//!
//! Each `BackendManifest` from `crate::manifest` resolves to one
//! `BackendInstance` variant. The instance owns the connection state
//! (reqwest client, MCP stdio process, OpenAI config) and is shared via
//! `Arc` across every binding that references it by name.

use std::sync::Arc;

use n3ur0n_adapters::openai::{OpenAIBackend, OpenAIConfig};

use crate::error::{NodeError, NodeResult};
use crate::manifest::{BackendKind, BackendManifest, McpServerConfig};

use super::http::HttpBackend;
use super::mcp::McpBackend;

/// Discriminated handle to a live upstream backend. Each variant wraps
/// the concrete connection / config type the bindings of that kind need.
#[derive(Clone, Debug)]
pub enum BackendInstance {
    /// OpenAI / Ollama / vLLM / llama.cpp compatible chat endpoint. Wraps
    /// the existing `OpenAIBackend` from `n3ur0n_adapters`.
    OpenAI(Arc<OpenAIBackend>),
    /// HTTP base — only carries shared base_url + default headers. The
    /// actual reqwest client is held inside the `HttpBackend`.
    Http(Arc<HttpBackend>),
    /// MCP server (stdio for v0.3.0). Wraps the stdio client + tools list.
    Mcp(Arc<McpBackend>),
}

/// Materialise one backend manifest into a `BackendInstance`.
///
/// For `mcp_server`, this is potentially expensive (spawns the subprocess)
/// — wrap the future at the caller site; this fn returns synchronously for
/// `openai_compat` and `http_base`, and `Mcp` lazily defers spawn to first
/// invocation today (Phase 2 stub; eager-warmup arrives in Phase 4 with
/// the lifecycle table).
pub fn build_backend_instance(m: &BackendManifest) -> NodeResult<BackendInstance> {
    match &m.kind {
        BackendKind::OpenAICompat(cfg) => {
            let cfg = OpenAIConfig {
                base_url: cfg.base_url.clone(),
                default_model: cfg.default_model.clone(),
                api_key: if cfg.api_key.is_empty() {
                    None
                } else {
                    Some(cfg.api_key.clone())
                },
                description: None,
                allow_model_override: false,
            };
            let backend = OpenAIBackend::new(cfg)
                .map_err(|e| NodeError::InvalidPayload(format!("openai backend init: {e}")))?;
            Ok(BackendInstance::OpenAI(Arc::new(backend)))
        }
        BackendKind::HttpBase(cfg) => {
            let backend = HttpBackend::new(cfg.clone())
                .map_err(|e| NodeError::InvalidPayload(format!("http backend init: {e}")))?;
            Ok(BackendInstance::Http(Arc::new(backend)))
        }
        BackendKind::McpServer(cfg) => {
            let backend = McpBackend::new(cfg.clone());
            Ok(BackendInstance::Mcp(Arc::new(backend)))
        }
    }
    .inspect(|inst| {
        tracing::debug!(name = %m.name, kind = ?backend_kind_tag(inst), "backend instance built");
    })
}

fn backend_kind_tag(inst: &BackendInstance) -> &'static str {
    match inst {
        BackendInstance::OpenAI(_) => "openai_compat",
        BackendInstance::Http(_) => "http_base",
        BackendInstance::Mcp(_) => "mcp_server",
    }
}

#[allow(dead_code)] // used by McpServerConfig type re-export; silences warning
fn _force_use(_: McpServerConfig) {}
