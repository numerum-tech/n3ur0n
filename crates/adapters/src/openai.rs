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

fn build_request(args: &Value, default_model: &str) -> AdapterResult<Value> {
    // Convenience: a string `prompt` becomes a single user message.
    if let Some(prompt) = args.get("prompt").and_then(|v| v.as_str()) {
        let mut req = json!({
            "model": args.get("model").cloned().unwrap_or_else(|| Value::String(default_model.to_string())),
            "messages": [{"role": "user", "content": prompt}],
            "stream": false,
        });
        if let Some(tools) = args.get("tools") {
            req["tools"] = tools.clone();
        }
        return Ok(req);
    }
    if args.get("messages").is_none() {
        return Err(AdapterError::Backend(
            "invoke args must contain either `prompt` (string) or `messages` (array)".into(),
        ));
    }
    let mut obj = args.clone();
    if let Value::Object(map) = &mut obj {
        // We don't support streaming over the protocol envelope yet — force false.
        map.insert("stream".into(), Value::Bool(false));
    }
    Ok(obj)
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
