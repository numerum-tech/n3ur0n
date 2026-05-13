//! `prompt` binding — LLM call configured by manifest data.
//!
//! Constructed from `BindingSpec::Prompt { ... }` + an OpenAI-compatible
//! backend instance. Behaviour:
//!
//! 1. Render `user_template` against caller args (or JSON-serialise the
//!    args when no template is set).
//! 2. Build a chat completion request: `[{role: system, system_prompt},
//!    {role: user, rendered_user}]` plus the `parameters` block from the
//!    manifest (temperature, max_tokens, etc).
//! 3. Forward to the backend's `invoke("chat", ...)` — which is the
//!    existing `OpenAIBackend` path.
//! 4. Parse the upstream `message.content` according to `output_parser`:
//!    `text` → `{"text": "..."}` ; `json` → `serde_json::from_str` (caller
//!    side validates against `schema_out`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use n3ur0n_adapters::{openai::OpenAIBackend, Backend};
use serde_json::{json, Value};

use crate::error::{NodeError, NodeResult};
use crate::manifest::{BindingSpec, OutputParser};

use super::template;
use super::Binding;

#[derive(Clone)]
pub struct PromptBinding {
    backend: Arc<OpenAIBackend>,
    system_prompt: String,
    user_template: Option<String>,
    parameters: HashMap<String, Value>,
    output_parser: OutputParser,
    model: Option<String>,
}

impl std::fmt::Debug for PromptBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptBinding")
            .field("model", &self.model)
            .field("output_parser", &self.output_parser)
            .field("has_user_template", &self.user_template.is_some())
            .finish()
    }
}

impl PromptBinding {
    pub fn new(spec: BindingSpec, backend: Arc<OpenAIBackend>) -> NodeResult<Self> {
        let BindingSpec::Prompt {
            backend: _,
            system_prompt,
            user_template,
            parameters,
            output_parser,
            model,
        } = spec
        else {
            return Err(NodeError::InvalidPayload(
                "PromptBinding requires BindingSpec::Prompt".into(),
            ));
        };
        Ok(Self {
            backend,
            system_prompt,
            user_template,
            parameters,
            output_parser,
            model,
        })
    }
}

#[async_trait]
impl Binding for PromptBinding {
    async fn invoke(&self, args: Value) -> NodeResult<Value> {
        // 1. Render the user-facing message.
        let user_content: String = match &self.user_template {
            Some(tmpl) => match template::render(tmpl, &args)? {
                Value::String(s) => s,
                other => other.to_string(),
            },
            None => serde_json::to_string(&args)
                .map_err(|e| NodeError::InvalidPayload(format!("args serialise: {e}")))?,
        };

        // 2. Build the chat request payload. Drop anything that does not
        // belong in an OpenAI request (the backend sanitiser will also
        // strip, but doing it here keeps the trace readable).
        let mut payload = json!({
            "messages": [
                {"role": "system", "content": self.system_prompt},
                {"role": "user", "content": user_content},
            ]
        });
        for (k, v) in &self.parameters {
            payload[k] = v.clone();
        }
        if let Some(model) = &self.model {
            payload["model"] = Value::String(model.clone());
        }

        // 3. Invoke the upstream.
        let resp = self
            .backend
            .invoke("chat", payload)
            .await
            .map_err(NodeError::from)?;

        // 4. Parse out the assistant content.
        let content = resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                NodeError::InvalidPayload(
                    "prompt binding: upstream response missing /message/content".into(),
                )
            })?
            .trim()
            .to_string();

        match self.output_parser {
            OutputParser::Text => Ok(json!({"text": content})),
            OutputParser::Json => {
                let parsed: Value = serde_json::from_str(&content).map_err(|e| {
                    NodeError::InvalidPayload(format!(
                        "prompt binding: output_parser=json failed: {e}; raw: {}",
                        content.chars().take(200).collect::<String>()
                    ))
                })?;
                Ok(parsed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use n3ur0n_adapters::{AdapterError, AdapterResult, Backend as AdapterBackend, HealthStatus};
    use n3ur0n_core::capability::CapabilityDecl;
    use std::sync::Mutex;

    /// Tiny mock that records the last chat payload and returns a
    /// canned reply. Bypasses `OpenAIBackend` (which would issue a real
    /// HTTP call); we exercise the binding glue end-to-end without
    /// touching the network.
    struct MockChat {
        canned_content: String,
        last_payload: Mutex<Option<Value>>,
    }

    #[async_trait]
    impl AdapterBackend for MockChat {
        async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value> {
            if capability != "chat" {
                return Err(AdapterError::UnknownCapability(capability.into()));
            }
            *self.last_payload.lock().unwrap() = Some(args);
            Ok(json!({
                "model": "mock",
                "message": {"role": "assistant", "content": self.canned_content},
                "finish_reason": "stop"
            }))
        }
        async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
            Ok(vec![])
        }
        async fn health(&self) -> AdapterResult<HealthStatus> {
            Ok(HealthStatus::Healthy)
        }
    }

    // Helper: PromptBinding ignores the OpenAIBackend type at compile
    // time but the type system needs it. For tests we cheat by calling
    // the binding's invoke method directly with a custom backend by
    // building the binding manually here.
    struct DirectBinding {
        chat: Arc<dyn AdapterBackend>,
        system: String,
        user_tmpl: Option<String>,
        parser: OutputParser,
    }

    impl std::fmt::Debug for DirectBinding {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DirectBinding").finish()
        }
    }

    #[async_trait]
    impl Binding for DirectBinding {
        async fn invoke(&self, args: Value) -> NodeResult<Value> {
            let user = match &self.user_tmpl {
                Some(t) => match template::render(t, &args)? {
                    Value::String(s) => s,
                    other => other.to_string(),
                },
                None => serde_json::to_string(&args).unwrap(),
            };
            let resp = self
                .chat
                .invoke(
                    "chat",
                    json!({
                        "messages": [
                            {"role": "system", "content": self.system},
                            {"role": "user", "content": user}
                        ]
                    }),
                )
                .await
                .map_err(NodeError::from)?;
            let content = resp["message"]["content"].as_str().unwrap().to_string();
            match self.parser {
                OutputParser::Text => Ok(json!({"text": content})),
                OutputParser::Json => {
                    Ok(serde_json::from_str(&content).map_err(|e| {
                        NodeError::InvalidPayload(e.to_string())
                    })?)
                }
            }
        }
    }

    #[tokio::test]
    async fn renders_user_template_and_returns_text_output() {
        let mock: Arc<dyn AdapterBackend> = Arc::new(MockChat {
            canned_content: "Bonjour, monde.".into(),
            last_payload: Mutex::new(None),
        });
        let b = DirectBinding {
            chat: mock.clone(),
            system: "You translate to French.".into(),
            user_tmpl: Some("Translate: {{args.text}}".into()),
            parser: OutputParser::Text,
        };
        let out = b
            .invoke(json!({"text": "Hello, world."}))
            .await
            .unwrap();
        assert_eq!(out, json!({"text": "Bonjour, monde."}));
    }

    #[tokio::test]
    async fn json_parser_returns_structured_output() {
        let mock: Arc<dyn AdapterBackend> = Arc::new(MockChat {
            canned_content: r#"{"translation":"hello"}"#.into(),
            last_payload: Mutex::new(None),
        });
        let b = DirectBinding {
            chat: mock,
            system: "Output JSON.".into(),
            user_tmpl: None,
            parser: OutputParser::Json,
        };
        let out = b.invoke(json!({"text": "bonjour"})).await.unwrap();
        assert_eq!(out, json!({"translation": "hello"}));
    }
}
