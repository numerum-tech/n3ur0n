//! `mcp` binding — call a tool on an MCP server.
//!
//! Backend = one MCP server (stdio subprocess for v0.3.0). Binding = one
//! `tool_name` exposed by that server. Many bindings share the same
//! subprocess via `McpBackend`.
//!
//! Lazy connection: the subprocess is spawned on first invocation. Eager
//! warmup arrives in Phase 4 with the lifecycle table.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::warn;

use crate::error::{NodeError, NodeResult};
use crate::manifest::{BindingSpec, McpServerConfig, McpTransport};

use super::mcp_client::{McpSession, SharedSession};
use super::template;
use super::Binding;

/// Live MCP backend handle. Holds the server config + a lazily-spawned
/// session. `Mutex<Option<SharedSession>>` so first-invocation spawn is
/// serialised across concurrent callers.
pub struct McpBackend {
    config: McpServerConfig,
    session: Mutex<Option<SharedSession>>,
}

impl std::fmt::Debug for McpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBackend")
            .field("command", &self.config.command)
            .field("transport", &self.config.transport)
            .finish()
    }
}

impl McpBackend {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            session: Mutex::new(None),
        }
    }

    /// Return the shared session, spawning the subprocess on first call.
    /// Subsequent calls hand back the existing handle.
    async fn ensure_session(&self) -> NodeResult<SharedSession> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        if !matches!(self.config.transport, McpTransport::Stdio) {
            return Err(NodeError::InvalidPayload(
                "mcp backend: only stdio transport is supported in v0.3.0".into(),
            ));
        }
        let session = McpSession::spawn(&self.config.command, &self.config.args, &self.config.env)
            .await
            .map_err(|e| NodeError::InvalidPayload(format!("mcp spawn: {e}")))?;
        let shared = SharedSession::new(session);
        *guard = Some(shared.clone());
        Ok(shared)
    }
}

#[derive(Clone)]
pub struct McpBinding {
    backend: Arc<McpBackend>,
    tool_name: String,
    arg_mapping: HashMap<String, Value>,
    #[allow(dead_code)] // applied post-result in v0.3.1 if needed
    result_mapping: HashMap<String, Value>,
}

impl std::fmt::Debug for McpBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpBinding")
            .field("tool_name", &self.tool_name)
            .finish()
    }
}

impl McpBinding {
    pub fn new(spec: BindingSpec, backend: Arc<McpBackend>) -> NodeResult<Self> {
        let BindingSpec::Mcp {
            backend: _,
            tool_name,
            arg_mapping,
            result_mapping,
        } = spec
        else {
            return Err(NodeError::InvalidPayload(
                "McpBinding requires BindingSpec::Mcp".into(),
            ));
        };
        Ok(Self {
            backend,
            tool_name,
            arg_mapping,
            result_mapping,
        })
    }

    /// Translate caller args according to `arg_mapping`. When the mapping
    /// is empty, pass args through unchanged.
    fn map_args(&self, args: Value) -> NodeResult<Value> {
        if self.arg_mapping.is_empty() {
            return Ok(args);
        }
        let mut out = serde_json::Map::with_capacity(self.arg_mapping.len());
        for (target_key, tmpl) in &self.arg_mapping {
            // `arg_mapping = { mcp_key = "{{args.our_key}}" }`
            let rendered = match tmpl {
                Value::String(s) => template::render(s, &args)?,
                other => template::render_value(other, &args)?,
            };
            out.insert(target_key.clone(), rendered);
        }
        Ok(Value::Object(out))
    }
}

#[async_trait]
impl Binding for McpBinding {
    fn kind(&self) -> &'static str { "mcp" }

    async fn invoke(&self, args: Value) -> NodeResult<Value> {
        let mapped = self.map_args(args)?;
        let session = self.backend.ensure_session().await?;
        let session = session.0.clone();
        let mut guard = session.lock().await;
        let raw = guard
            .call_tool(&self.tool_name, mapped)
            .await
            .map_err(|e| NodeError::InvalidPayload(format!("mcp call_tool: {e}")))?;
        // MCP `tools/call` result shape: { content: [{type: "text"|"json"|..., ...}], isError?: bool }.
        // We surface the result as-is for v0.3.0 and let cap authors
        // shape it via `schema_out` + (later) `result_mapping`. Log a
        // warning on isError=true so the trace surfaces it.
        if raw.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            warn!(tool = %self.tool_name, raw = %raw, "mcp tool reported isError=true");
        }
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_spec() -> BindingSpec {
        BindingSpec::Mcp {
            backend: "test".into(),
            tool_name: "echo".into(),
            arg_mapping: HashMap::new(),
            result_mapping: HashMap::new(),
        }
    }

    #[test]
    fn builds_from_spec() {
        let cfg = McpServerConfig {
            transport: McpTransport::Stdio,
            command: "true".into(),
            args: vec![],
            env: HashMap::new(),
        };
        let backend = Arc::new(McpBackend::new(cfg));
        let binding = McpBinding::new(make_spec(), backend).unwrap();
        assert_eq!(binding.tool_name, "echo");
    }

    #[test]
    fn map_args_passthrough_when_empty() {
        let cfg = McpServerConfig {
            transport: McpTransport::Stdio,
            command: "true".into(),
            args: vec![],
            env: HashMap::new(),
        };
        let backend = Arc::new(McpBackend::new(cfg));
        let b = McpBinding::new(make_spec(), backend).unwrap();
        let v = b.map_args(json!({"x": 1})).unwrap();
        assert_eq!(v, json!({"x": 1}));
    }

    #[test]
    fn map_args_renames_keys() {
        let mut mapping = HashMap::new();
        mapping.insert("mcp_query".into(), Value::String("{{args.q}}".into()));
        mapping.insert("limit".into(), Value::String("{{args.n}}".into()));
        let spec = BindingSpec::Mcp {
            backend: "test".into(),
            tool_name: "search".into(),
            arg_mapping: mapping,
            result_mapping: HashMap::new(),
        };
        let cfg = McpServerConfig {
            transport: McpTransport::Stdio,
            command: "true".into(),
            args: vec![],
            env: HashMap::new(),
        };
        let backend = Arc::new(McpBackend::new(cfg));
        let b = McpBinding::new(spec, backend).unwrap();
        let out = b
            .map_args(json!({"q": "leak", "n": 10}))
            .unwrap();
        assert_eq!(out["mcp_query"], "leak");
        assert_eq!(out["limit"], 10);
    }

    /// Live subprocess test gated behind an env flag: set
    /// `N3UR0N_MCP_LIVE_TEST=1` and provide a python3 in PATH. The fixture
    /// implements initialize + tools/list + tools/call(echo).
    #[tokio::test]
    async fn live_stdio_echo_when_env_flag_set() {
        if std::env::var("N3UR0N_MCP_LIVE_TEST").is_err() {
            return;
        }
        // CARGO_MANIFEST_DIR points to `crates/node/` at compile time.
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mock_mcp_server.py");
        if !fixture.exists() {
            eprintln!("fixture missing at {fixture:?} — skipping");
            return;
        }
        let cfg = McpServerConfig {
            transport: McpTransport::Stdio,
            command: "python3".into(),
            args: vec![fixture.to_string_lossy().to_string()],
            env: HashMap::new(),
        };
        let backend = Arc::new(McpBackend::new(cfg));
        let binding = McpBinding::new(make_spec(), backend).unwrap();
        let out = binding.invoke(json!({"text": "hi"})).await.unwrap();
        // Fixture echoes args inside an MCP `content` envelope.
        assert!(out.get("content").is_some());
    }
}
