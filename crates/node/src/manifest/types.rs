//! Types backing the v0.3 manifest format.
//!
//! Two files, two top-level structs:
//!
//! - `BackendManifest` ← `backends/<name>.toml`
//! - `CapabilityManifest` ← `caps/<name>.toml`
//!
//! Each capability references a backend by name (`binding.backend = "..."`).
//! A backend defined once is reusable by N capabilities.

use std::collections::HashMap;

use n3ur0n_core::capability::CapabilityDecl;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Wrapper for parsing — TOML carries a `[manifest]` header that we strip
// after version check.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ManifestHeader {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct BackendFile {
    pub manifest: ManifestHeader,
    pub backend: BackendDescriptor,
    #[serde(flatten)]
    pub extras: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct BackendDescriptor {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct CapFile {
    pub manifest: ManifestHeader,
    pub descriptor: CapabilityDecl,
    pub binding: BindingHeader,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct BindingHeader {
    #[serde(rename = "type")]
    pub kind: String,
    pub backend: String,
    pub prompt: Option<PromptSection>,
    pub mcp: Option<McpSection>,
    pub http: Option<HttpSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct PromptSection {
    pub system_prompt: String,
    #[serde(default)]
    pub user_template: Option<String>,
    #[serde(default)]
    pub parameters: HashMap<String, Value>,
    #[serde(default)]
    pub output_parser: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct McpSection {
    pub tool_name: String,
    #[serde(default)]
    pub arg_mapping: HashMap<String, Value>,
    #[serde(default)]
    pub result_mapping: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct HttpSection {
    pub url_template: String,
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body_template: Option<Value>,
    #[serde(default)]
    pub response_path: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Public, normalised types — what the rest of the runtime consumes.
// ---------------------------------------------------------------------------

/// A parsed `backends/<name>.toml`. Always carries a name and one
/// kind-specific config blob.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendManifest {
    pub name: String,
    pub kind: BackendKind,
}

/// Discriminated union of backend kinds. Each variant carries the
/// kind-specific config struct from below.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendKind {
    /// OpenAI / Ollama / vLLM / llama.cpp compatible chat completions
    /// endpoint. The most common backend.
    OpenAICompat(OpenAICompatConfig),
    /// MCP server (stdio or HTTP/SSE) exposing one or more tools.
    McpServer(McpServerConfig),
    /// Base URL + default headers for HTTP-binding capabilities.
    HttpBase(HttpBaseConfig),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAICompatConfig {
    pub base_url: String,
    pub default_model: String,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    pub transport: McpTransport,
    /// stdio command (exec path); HTTP/SSE base URL.
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    HttpSse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpBaseConfig {
    pub base_url: String,
    pub headers: HashMap<String, String>,
}

/// A parsed `caps/<name>.toml`. Carries the descriptor (which becomes the
/// wire `CapabilityDecl`) and a binding spec referencing a backend by name.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityManifest {
    pub descriptor: CapabilityDecl,
    pub binding: BindingSpec,
}

/// Discriminated union of binding kinds. The `backend` field on every
/// variant references a `BackendManifest.name`.
#[derive(Debug, Clone, PartialEq)]
pub enum BindingSpec {
    /// LLM call configured as data: system prompt + user template +
    /// optional parameters. Cap author authors prose; the runtime forwards
    /// it to the referenced backend (typically `openai_compat`).
    Prompt {
        backend: String,
        system_prompt: String,
        user_template: Option<String>,
        parameters: HashMap<String, Value>,
        output_parser: OutputParser,
        model: Option<String>,
    },
    Mcp {
        backend: String,
        tool_name: String,
        arg_mapping: HashMap<String, Value>,
        result_mapping: HashMap<String, Value>,
    },
    Http {
        backend: String,
        url_template: String,
        method: HttpMethod,
        headers: HashMap<String, String>,
        body_template: Option<Value>,
        response_path: Option<String>,
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputParser {
    /// Wrap upstream content in `{"text": "..."}`.
    Text,
    /// Parse upstream content as JSON; must validate against `schema_out`.
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl BindingSpec {
    /// Backend reference used by this binding. Useful for the registry
    /// resolver.
    pub fn backend(&self) -> &str {
        match self {
            BindingSpec::Prompt { backend, .. } => backend,
            BindingSpec::Mcp { backend, .. } => backend,
            BindingSpec::Http { backend, .. } => backend,
        }
    }
}
