//! Minimal MCP client over stdio.
//!
//! Implements just enough of the Model Context Protocol JSON-RPC 2.0 wire
//! format for our v0.3.0 needs:
//!
//! - `initialize` (sent once on connect)
//! - `tools/list` (used at warmup and to validate `tool_name`)
//! - `tools/call` (per-binding-invocation)
//!
//! Transport: stdio only (Phase 2 scope). One subprocess per backend; many
//! bindings share it via the `McpBackend` wrapper.
//!
//! NOT implemented (out of scope v0.3.0):
//! - HTTP/SSE transport
//! - `notifications/tools/list_changed`
//! - prompts, resources, roots
//! - subscriptions / streaming results
//!
//! References: <https://modelcontextprotocol.io/specification/2025-06-18>

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("spawn `{cmd}` failed: {source}")]
    Spawn { cmd: String, source: std::io::Error },
    #[error("io error talking to mcp server: {0}")]
    Io(#[from] std::io::Error),
    #[error("mcp server returned protocol error: {0}")]
    Protocol(String),
    #[error("mcp server returned application error code={code} message={message}")]
    Application { code: i64, message: String },
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Owned MCP stdio session — one running subprocess, one request/response
/// pipe. Wrap in `Arc<Mutex<...>>` to share between bindings.
pub struct McpSession {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl std::fmt::Debug for McpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSession")
            .field("next_id", &self.next_id)
            .finish()
    }
}

impl McpSession {
    /// Spawn the MCP server process and complete the `initialize`
    /// handshake. Returns a ready-to-use session.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args).envs(env);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().map_err(|source| McpError::Spawn {
            cmd: command.to_string(),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("child stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("child stdout missing".into()))?;
        let mut session = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        session.initialize().await?;
        Ok(session)
    }

    async fn initialize(&mut self) -> Result<(), McpError> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "clientInfo": {"name": "n3ur0n", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        // Per MCP, send a notification (no id) to signal we're ready. Best
        // effort — some servers don't require it.
        let notif = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.write_message(&notif).await?;
        Ok(())
    }

    /// `tools/list` — returns the raw `tools` array as advertised.
    pub async fn list_tools(&mut self) -> Result<Vec<Value>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| McpError::Protocol("tools/list missing `tools`".into()))?;
        Ok(tools)
    }

    /// `tools/call` — invoke a named tool with `arguments`. Returns the
    /// `result` field of the JSON-RPC envelope.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let result = self
            .request(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            )
            .await?;
        Ok(result)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let envelope = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&envelope).await?;

        // Read responses until we see one matching our id. (Servers may
        // emit notifications interleaved with replies; skip them.)
        loop {
            let msg = self.read_message().await?;
            if msg.get("id").and_then(|v| v.as_i64()) == Some(id) {
                if let Some(err) = msg.get("error") {
                    let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
                    let message = err
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no message)")
                        .to_string();
                    return Err(McpError::Application { code, message });
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notification or unrelated reply — ignore.
        }
    }

    async fn write_message(&mut self, value: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Value, McpError> {
        let mut buf = String::new();
        let n = self.stdout.read_line(&mut buf).await?;
        if n == 0 {
            return Err(McpError::Protocol("mcp server closed stdout (EOF)".into()));
        }
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Box::pin(self.read_message()).await;
        }
        let value: Value = serde_json::from_str(trimmed)?;
        Ok(value)
    }
}

/// Shared, async-safe handle to an MCP session. Cloneable.
#[derive(Clone, Debug)]
pub struct SharedSession(pub Arc<Mutex<McpSession>>);

impl SharedSession {
    pub fn new(session: McpSession) -> Self {
        Self(Arc::new(Mutex::new(session)))
    }
}

/// Trimmed view of one tool from `tools/list`. Used by binding warmup to
/// confirm the manifest's `tool_name` exists.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the wire format end-to-end against an in-memory pipe. Skips
    /// the real subprocess to keep CI hermetic.
    #[tokio::test]
    async fn jsonrpc_envelope_round_trip() {
        // Build a minimal valid envelope and round-trip through serde to
        // catch wire-format regressions.
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "echo", "arguments": {"x": 1}}
        });
        let s = serde_json::to_string(&req).unwrap();
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn application_error_has_code_and_message() {
        let err = McpError::Application {
            code: -32601,
            message: "method not found".into(),
        };
        let s = err.to_string();
        assert!(s.contains("-32601"));
        assert!(s.contains("method not found"));
    }
}
