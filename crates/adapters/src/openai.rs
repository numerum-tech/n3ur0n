//! OpenAI-compatible backend.
//!
//! Speaks the `/v1/chat/completions` shape — works with OpenAI itself, with
//! Ollama (`http://localhost:11434/v1`), with `llama.cpp` server, vLLM, and
//! any other server that implements the chat completions endpoint.
//!
//! v0.1 exposes a single capability `chat` mapped 1:1 onto the upstream
//! endpoint. The capability declares the request/response shape verbatim so
//! that callers can use the same payload they would give OpenAI.

use std::time::Duration;

use async_trait::async_trait;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, instrument};

use crate::{AdapterError, AdapterResult, Backend, HealthStatus};

const CHAT_CAPABILITY: &str = "chat";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Static configuration for [`OpenAIBackend`].
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    /// Base URL **without** the `/v1` suffix or trailing slash.
    /// For Ollama: `http://localhost:11434`.
    /// For OpenAI: `https://api.openai.com`.
    pub base_url: String,
    /// Default model name. Overridable in each `invoke` payload.
    pub default_model: String,
    /// Optional bearer token. Sent as `Authorization: Bearer <token>` if set.
    pub api_key: Option<String>,
    /// Optional capability description override.
    pub description: Option<String>,
}

impl OpenAIConfig {
    /// Convenience for Ollama running on localhost.
    pub fn ollama_local(model: impl Into<String>) -> Self {
        Self {
            base_url: "http://localhost:11434".into(),
            default_model: model.into(),
            api_key: None,
            description: None,
        }
    }
}

/// Backend impl over an OpenAI-compatible chat completions endpoint.
pub struct OpenAIBackend {
    config: OpenAIConfig,
    client: Client,
}

impl std::fmt::Debug for OpenAIBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIBackend")
            .field("base_url", &self.config.base_url)
            .field("default_model", &self.config.default_model)
            .field("authenticated", &self.config.api_key.is_some())
            .finish()
    }
}

impl OpenAIBackend {
    /// Build a new backend.
    pub fn new(config: OpenAIConfig) -> AdapterResult<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .user_agent("n3ur0n-adapter/0.1")
            .build()
            .map_err(|e| AdapterError::Transport(e.to_string()))?;
        Ok(Self { config, client })
    }

    fn chat_url(&self) -> String {
        format!("{}/v1/chat/completions", self.config.base_url.trim_end_matches('/'))
    }

    fn models_url(&self) -> String {
        format!("{}/v1/models", self.config.base_url.trim_end_matches('/'))
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatMessage {
    role: String,
    /// `content` may be `null` when the assistant only emits tool_calls.
    #[serde(default)]
    content: Option<String>,
    /// Native tool-calling output (OpenAI-compatible models / Ollama ≥ 0.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletion {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<ChatChoice>,
}

#[async_trait]
impl Backend for OpenAIBackend {
    #[instrument(skip(self, args), fields(capability = %capability))]
    async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value> {
        if capability != CHAT_CAPABILITY {
            return Err(AdapterError::UnknownCapability(capability.to_string()));
        }

        // Accept either {"messages": [...]} (OpenAI native) or
        // {"prompt": "..."} (convenience for the smoke test).
        let mut request = build_request(&args, &self.config.default_model)?;
        if request.get("model").is_none() {
            request["model"] = Value::String(self.config.default_model.clone());
        }
        debug!(target: "n3ur0n_adapters::openai", url = %self.chat_url(), "POST chat completions");

        let resp = self
            .auth(self.client.post(self.chat_url()).json(&request))
            .send()
            .await
            .map_err(|e| AdapterError::Transport(e.to_string()))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(AdapterError::Backend(format!(
                "{} returned {}: {}",
                self.chat_url(),
                status,
                String::from_utf8_lossy(&bytes)
            )));
        }

        let parsed: ChatCompletion = serde_json::from_slice(&bytes)?;
        let first = parsed.choices.into_iter().next().ok_or_else(|| {
            AdapterError::Backend("upstream returned no choices".into())
        })?;
        let mut message = json!({
            "role": first.message.role,
            "content": first.message.content,
        });
        if let Some(tc) = first.message.tool_calls {
            message["tool_calls"] = tc;
        }
        Ok(json!({
            "model": parsed.model,
            "message": message,
            "finish_reason": first.finish_reason,
        }))
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        let description = self
            .config
            .description
            .clone()
            .unwrap_or_else(|| {
                format!(
                    "OpenAI-compatible chat completion via {} (default model: {}).",
                    self.config.base_url, self.config.default_model
                )
            });
        Ok(vec![CapabilityDecl {
            name: CHAT_CAPABILITY.into(),
            description,
            schema_in: chat_schema_in(),
            schema_out: chat_schema_out(),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec!["chat".into(), "llm".into()],
            lobe_ids: vec![],
        }])
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        // GET /v1/models is the standard probe; treat 401 as Degraded
        // (server reachable, auth missing) rather than Unhealthy.
        let resp = self.auth(self.client.get(self.models_url())).send().await;
        match resp {
            Ok(r) if r.status().is_success() => Ok(HealthStatus::Healthy),
            Ok(r) if r.status().as_u16() == 401 => Ok(HealthStatus::Degraded),
            Ok(_) => Ok(HealthStatus::Degraded),
            Err(_) => Ok(HealthStatus::Unhealthy),
        }
    }
}

/// Allowlist of fields a `chat` cap caller may set in args. Anything else
/// is dropped to keep the upstream Ollama / OpenAI request safe.
///
/// Why: callers (planner LLMs, mostly) sometimes dump their own context —
/// notably a `tools: [...]` array intended for their planner step — into
/// the args of a downstream `chat` cap whose model (qwen2.5:0.5b) does not
/// support tool-calling. The upstream then 500s. This sanitiser keeps only
/// the fields the chat cap actually advertises.
const CHAT_ARG_ALLOWLIST: &[&str] = &[
    "prompt",
    "messages",
    "temperature",
    "max_tokens",
    "top_p",
    "stop",
    // Tool-calling fields are explicitly allowed — the planner uses
    // OpenAIBackend for its own LLM calls and needs to pass `tools`
    // through. If the operator exposes `chat` backed by a non-tool-aware
    // model, the upstream is expected to ignore unknown fields (Ollama
    // does); a strict gateway can be added later.
    "tools",
    "tool_choice",
];

fn build_request(args: &Value, default_model: &str) -> AdapterResult<Value> {
    // Apply allowlist first — drop tools / tool_choice / model overrides /
    // anything else exotic.
    let sanitised = sanitise_chat_args(args);

    // Convenience: a string `prompt` becomes a single user message.
    if let Some(prompt) = sanitised.get("prompt").and_then(|v| v.as_str()) {
        return Ok(json!({
            // Lock the model to the operator-configured default. Callers
            // cannot override — they may hallucinate a model name (seen
            // with llama3.1:8b emitting "text-davinci-003"), which would
            // 500 against an Ollama upstream that has not pulled that
            // model. The operator picked what to serve; honour that.
            "model": default_model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": false,
        }));
    }
    if sanitised.get("messages").is_none() {
        return Err(AdapterError::Backend(
            "invoke args must contain either `prompt` (string) or `messages` (array)".into(),
        ));
    }
    let mut obj = sanitised;
    if let Value::Object(map) = &mut obj {
        // Lock model regardless of what the client sent.
        map.insert("model".into(), Value::String(default_model.to_string()));
        // We don't support streaming over the protocol envelope yet — force false.
        map.insert("stream".into(), Value::Bool(false));
        // NOTE: messages content is forwarded as-is. `tool_calls` and
        // `role: tool` entries inside `messages` are intentionally
        // preserved so the planner's own iterative loop can show the LLM
        // its prior tool exchanges. Operators exposing the `chat` cap with
        // a non-tool-aware model (qwen2.5:0.5b, etc.) should pick a model
        // that can ignore these fields gracefully — Ollama 0.4+ does so.
    }
    Ok(obj)
}

fn sanitise_chat_args(args: &Value) -> Value {
    match args {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for k in CHAT_ARG_ALLOWLIST {
                if let Some(v) = map.get(*k) {
                    out.insert((*k).to_string(), v.clone());
                }
            }
            // Coerce `messages` into an array if a caller passed it as a
            // JSON-encoded string (observed: llama3.1:8b emitting
            // "messages": "[{...}]"). If the string isn't valid JSON or
            // doesn't decode to an array, drop it — the caller can fall
            // back to `prompt`.
            if let Some(v) = out.get("messages").cloned() {
                if let Value::String(raw) = &v {
                    let coerced = serde_json::from_str::<Value>(raw)
                        .ok()
                        .filter(|d| d.is_array());
                    match coerced {
                        Some(arr) => {
                            out.insert("messages".into(), arr);
                        }
                        None => {
                            // Not a valid JSON array — fall back to a
                            // single-user-message prompt and drop messages.
                            out.insert("prompt".into(), Value::String(raw.clone()));
                            out.remove("messages");
                        }
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn chat_schema_in() -> Value {
    json!({
        "type": "object",
        "oneOf": [
            {
                "required": ["prompt"],
                "properties": {
                    "prompt": {"type": "string"},
                    "model": {"type": "string"}
                }
            },
            {
                "required": ["messages"],
                "properties": {
                    "model": {"type": "string"},
                    "messages": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["role", "content"],
                            "properties": {
                                "role": {"enum": ["system", "user", "assistant", "tool"]},
                                "content": {"type": "string"}
                            }
                        }
                    },
                    "temperature": {"type": "number"},
                    "max_tokens": {"type": "integer"}
                }
            }
        ]
    })
}

fn chat_schema_out() -> Value {
    json!({
        "type": "object",
        "required": ["message"],
        "properties": {
            "model": {"type": ["string", "null"]},
            "message": {
                "type": "object",
                "required": ["role", "content"],
                "properties": {
                    "role": {"type": "string"},
                    "content": {"type": "string"}
                }
            },
            "finish_reason": {"type": ["string", "null"]}
        }
    })
}
