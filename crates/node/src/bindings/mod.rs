//! Capability bindings: the runtime side of cap manifests.
//!
//! Each `BindingSpec` from `crate::manifest` translates to one `Binding`
//! impl that knows how to actually invoke the upstream service. Bindings
//! are stateless w.r.t. caller args; long-lived state (e.g. MCP stdio
//! session, reqwest client, OpenAI backend) is held by the underlying
//! backend instance and shared across bindings that reference the same
//! backend.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::NodeResult;

pub mod backend;
pub mod http;
pub mod mcp;
pub mod mcp_client;
pub mod prompt;
pub mod template;

pub use backend::{BackendInstance, build_backend_instance};
pub use http::HttpBinding;
pub use mcp::McpBinding;
pub use prompt::PromptBinding;

/// One ready-to-invoke capability binding. Constructed once at registry
/// build time (or hot-reload) from a `BindingSpec` + the resolved backend
/// instance referenced by the spec.
#[async_trait]
pub trait Binding: Send + Sync + std::fmt::Debug {
    /// Invoke the binding with caller args. Returns the cap's output
    /// shaped to match `schema_out` declared on the capability.
    async fn invoke(&self, args: Value) -> NodeResult<Value>;
}

/// Build a binding for the given spec, consuming the backend instance it
/// references. Returns an error if the binding spec is incompatible with
/// the backend kind (e.g., a prompt binding referencing an mcp_server
/// backend).
pub fn build_binding(
    spec: &crate::manifest::BindingSpec,
    backend: &BackendInstance,
) -> NodeResult<Arc<dyn Binding>> {
    use crate::manifest::BindingSpec as BS;
    match (spec, backend) {
        (BS::Prompt { .. }, BackendInstance::OpenAI(b)) => {
            Ok(Arc::new(PromptBinding::new(spec.clone(), b.clone())?))
        }
        (BS::Http { .. }, BackendInstance::Http(b)) => {
            Ok(Arc::new(HttpBinding::new(spec.clone(), b.clone())?))
        }
        (BS::Mcp { .. }, BackendInstance::Mcp(b)) => {
            Ok(Arc::new(McpBinding::new(spec.clone(), b.clone())?))
        }
        // Mismatch — give a clear error so the operator knows which cap is
        // wired to the wrong backend kind.
        (BS::Prompt { backend, .. }, _) => Err(crate::error::NodeError::InvalidPayload(
            format!("prompt binding requires an `openai_compat` backend; `{backend}` is a different kind"),
        )),
        (BS::Http { backend, .. }, _) => Err(crate::error::NodeError::InvalidPayload(
            format!("http binding requires an `http_base` backend; `{backend}` is a different kind"),
        )),
        (BS::Mcp { backend, .. }, _) => Err(crate::error::NodeError::InvalidPayload(
            format!("mcp binding requires an `mcp_server` backend; `{backend}` is a different kind"),
        )),
    }
}
