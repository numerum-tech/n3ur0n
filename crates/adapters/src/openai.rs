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
use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample, NegativeExample};
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
    /// Default model name. Overridable per `invoke` only when `allow_model_override` is true.
    pub default_model: String,
    /// Optional bearer token. Sent as `Authorization: Bearer <token>` if set.
    pub api_key: Option<String>,
    /// Optional capability description override.
    pub description: Option<String>,
    /// When true, `invoke` may use a caller-supplied `model` in the payload.
    /// Default false — network-facing backends lock to `default_model`.
    pub allow_model_override: bool,
}

/// Normalize an OpenAI-compat `base_url` (host + optional port only).
///
/// Strips common mistaken suffixes such as Ollama's native `/api/generate`
/// path or a pre-appended `/v1`. The adapter always appends `/v1/chat/completions`.
pub fn normalize_openai_base_url(url: &str) -> String {
    let mut s = url.trim().trim_end_matches('/').to_string();
    loop {
        let before = s.clone();
        for suffix in ["/v1/chat/completions", "/api/generate", "/v1"] {
            if let Some(stripped) = s.strip_suffix(suffix) {
                s = stripped.trim_end_matches('/').to_string();
            }
        }
        if s == before {
            break;
        }
    }
    s
}

impl OpenAIConfig {
    /// Convenience for Ollama running on localhost.
    pub fn ollama_local(model: impl Into<String>) -> Self {
        Self {
            base_url: "http://localhost:11434".into(),
            default_model: model.into(),
            api_key: None,
            description: None,
            allow_model_override: false,
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
    pub fn new(mut config: OpenAIConfig) -> AdapterResult<Self> {
        config.base_url = normalize_openai_base_url(&config.base_url);
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .user_agent("n3ur0n-adapter/0.1")
            .build()
            .map_err(|e| AdapterError::Transport(e.to_string()))?;
        Ok(Self { config, client })
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
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
        let request = build_request(&args, &self.config)?;
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
        let first = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AdapterError::Backend("upstream returned no choices".into()))?;
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
        let description = self.config.description.clone().unwrap_or_else(|| {
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
            tags: vec![
                "chat".into(),
                "llm".into(),
                "text-generation".into(),
                "reasoning".into(),
            ],
            lobe_ids: vec![],
            examples: vec![
                CapabilityExample {
                    user_intent: "answer a free-form question or generate prose".into(),
                    args: json!({"prompt": "Write one haiku about autumn."}),
                    expected_output: json!({
                        "model": "llama3.1:8b",
                        "message": {
                            "role": "assistant",
                            "content": "Crimson leaves descend / Wind whispers a fading song / Stillness draws nearer"
                        },
                        "finish_reason": "stop"
                    }),
                },
                CapabilityExample {
                    user_intent: "translate text between human languages".into(),
                    args: json!({
                        "prompt": "Translate to French: 'Good morning, world.'"
                    }),
                    expected_output: json!({
                        "model": "llama3.1:8b",
                        "message": {
                            "role": "assistant",
                            "content": "Bonjour, monde."
                        },
                        "finish_reason": "stop"
                    }),
                },
                CapabilityExample {
                    user_intent: "multi-turn dialogue with system steering".into(),
                    args: json!({
                        "messages": [
                            {"role": "system", "content": "You are a concise assistant."},
                            {"role": "user", "content": "Summarise: distributed systems trade consistency for availability."}
                        ]
                    }),
                    expected_output: json!({
                        "model": "llama3.1:8b",
                        "message": {
                            "role": "assistant",
                            "content": "CAP-theorem tradeoff: pick C or A under partition."
                        },
                        "finish_reason": "stop"
                    }),
                },
            ],
            disambiguation: Some(
                "General-purpose text in / text out. Use for open-ended language \
tasks (translation, summarisation, reasoning, creative writing, Q&A). For \
deterministic transformations on strings (reverse, length, etc.) prefer the \
dedicated utility caps when available."
                    .into(),
            ),
            negative_examples: vec![
                NegativeExample {
                    user_intent: "reverse the characters in a string".into(),
                    why_not: "deterministic utility caps (e.g. `reverse`) are cheaper \
and exact; do not delegate trivial string ops to a chat model."
                        .into(),
                },
                NegativeExample {
                    user_intent: "pick a random number".into(),
                    why_not: "LLMs are not uniform RNGs; use `random_int` instead.".into(),
                },
            ],
            output_semantic: Some(
                "Assistant turn content (the model's reply) plus model id and \
finish reason; the meaningful payload is `message.content`."
                    .into(),
            ),
            version: "0.1.0".into(),
            languages: vec![],
            countries: vec![],
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
    "model",
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
    // v0.2 constrained-decoding fields. Best-effort propagation:
    // - `grammar`: llama.cpp / vLLM GBNF string. Honoured by llama.cpp
    //   natively; Ollama 0.4 ignores; OpenAI ignores.
    // - `response_format`: OpenAI ≥ 2024-08 json_schema mode. Honoured
    //   by OpenAI + vLLM (outlines); llama.cpp ignores.
    // - `format`: Ollama-native (string "json" or a JSON schema object).
    //   Kept for backwards compatibility with the v0.1 path.
    // The upstream silently ignores unknown fields — we don't gate by
    // backend kind here.
    "grammar",
    "response_format",
    "format",
];

fn build_request(args: &Value, config: &OpenAIConfig) -> AdapterResult<Value> {
    // Apply allowlist first — drop exotic fields. `model` is forwarded then
    // pinned or honoured in `apply_model_lock` depending on config.
    let sanitised = sanitise_chat_args(args);

    // Convenience: a string `prompt` becomes a single user message.
    if let Some(prompt) = sanitised.get("prompt").and_then(|v| v.as_str()) {
        return Ok(json!({
            // Prompt shorthand always locks model (smoke / network callers).
            "model": config.default_model,
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
        apply_model_lock(map, config);
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

/// Honour caller `model` only when `allow_model_override` is set (planner/direct).
fn apply_model_lock(map: &mut serde_json::Map<String, Value>, config: &OpenAIConfig) {
    if config.allow_model_override {
        let caller = map.get("model").and_then(|v| v.as_str());
        if caller.map(str::is_empty).unwrap_or(true) {
            map.insert("model".into(), Value::String(config.default_model.clone()));
        }
    } else {
        map.insert("model".into(), Value::String(config.default_model.clone()));
    }
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
            if let Some(Value::String(raw)) = out.get("messages").cloned() {
                let coerced = serde_json::from_str::<Value>(&raw)
                    .ok()
                    .filter(|d| d.is_array());
                match coerced {
                    Some(arr) => {
                        out.insert("messages".into(), arr);
                    }
                    None => {
                        // Not a valid JSON array — fall back to a
                        // single-user-message prompt and drop messages.
                        out.insert("prompt".into(), Value::String(raw));
                        out.remove("messages");
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
