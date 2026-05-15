//! `http` binding — forward to an HTTP endpoint declared in the manifest.
//!
//! A `HttpBackend` holds the shared `base_url` + default headers (auth
//! token, user-agent, etc) for one upstream service. Multiple capabilities
//! reference the same backend and stack their own `url_template`, method,
//! body, and `response_path` on top.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use serde_json_path::JsonPath;
use thiserror::Error;

use crate::error::{NodeError, NodeResult};
use crate::manifest::{BindingSpec, HttpBaseConfig, HttpMethod};

use super::template;
use super::Binding;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum HttpBackendError {
    #[error("invalid base_url `{0}`")]
    InvalidBaseUrl(String),
}

/// Live `http_base` backend instance. Wraps a configured `reqwest::Client`
/// and the shared base URL + default headers.
#[derive(Clone)]
pub struct HttpBackend {
    client: Client,
    base_url: String,
    default_headers: HashMap<String, String>,
}

impl std::fmt::Debug for HttpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpBackend")
            .field("base_url", &self.base_url)
            .field("default_headers", &self.default_headers.len())
            .finish()
    }
}

impl HttpBackend {
    pub fn new(cfg: HttpBaseConfig) -> Result<Self, HttpBackendError> {
        if cfg.base_url.trim().is_empty() {
            return Err(HttpBackendError::InvalidBaseUrl(cfg.base_url));
        }
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .user_agent("n3ur0n/0.3")
            .build()
            .expect("reqwest client builds with defaults");
        Ok(Self {
            client,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            default_headers: cfg.headers,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[derive(Clone)]
pub struct HttpBinding {
    backend: Arc<HttpBackend>,
    url_template: String,
    method: HttpMethod,
    headers: HashMap<String, String>,
    body_template: Option<Value>,
    response_path: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for HttpBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpBinding")
            .field("method", &self.method)
            .field("url_template", &self.url_template)
            .finish()
    }
}

impl HttpBinding {
    pub fn new(spec: BindingSpec, backend: Arc<HttpBackend>) -> NodeResult<Self> {
        let BindingSpec::Http {
            backend: _,
            url_template,
            method,
            headers,
            body_template,
            response_path,
            timeout_ms,
        } = spec
        else {
            return Err(NodeError::InvalidPayload(
                "HttpBinding requires BindingSpec::Http".into(),
            ));
        };
        let timeout = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);
        Ok(Self {
            backend,
            url_template,
            method,
            headers,
            body_template,
            response_path,
            timeout,
        })
    }
}

#[async_trait]
impl Binding for HttpBinding {
    fn kind(&self) -> &'static str { "http" }

    async fn invoke(&self, args: Value) -> NodeResult<Value> {
        // 1. Resolve URL: substitute `{{args.x}}` in the template, then
        // join with base. If the template begins with `http(s)://`, it's
        // an absolute override; otherwise prefix base.
        let rendered_url = match template::render(&self.url_template, &args)? {
            Value::String(s) => s,
            other => other.to_string(),
        };
        let url = if rendered_url.starts_with("http://") || rendered_url.starts_with("https://") {
            rendered_url
        } else {
            format!(
                "{}{}",
                self.backend.base_url,
                if rendered_url.starts_with('/') {
                    rendered_url.as_str().to_string()
                } else {
                    format!("/{rendered_url}")
                }
            )
        };

        // 2. Build the request.
        let reqwest_method = match self.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };
        let mut req = self
            .backend
            .client
            .request(reqwest_method, &url)
            .timeout(self.timeout);

        // Headers: backend defaults first, binding-level overrides win.
        for (k, v) in &self.backend.default_headers {
            req = req.header(k, v);
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        // 3. Body — render template tree with args.
        if let Some(body_tmpl) = &self.body_template {
            let rendered = template::render_value(body_tmpl, &args)?;
            req = req.json(&rendered);
        }

        // 4. Send.
        let resp = req
            .send()
            .await
            .map_err(|e| NodeError::Adapter(n3ur0n_adapters::AdapterError::Transport(e.to_string())))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| NodeError::Adapter(n3ur0n_adapters::AdapterError::Transport(e.to_string())))?;
        if !status.is_success() {
            return Err(NodeError::Adapter(n3ur0n_adapters::AdapterError::Backend(
                format!(
                    "http {} returned {}: {}",
                    url,
                    status,
                    String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>()
                ),
            )));
        }

        let body: Value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).map_err(|e| {
                NodeError::Adapter(n3ur0n_adapters::AdapterError::Backend(format!(
                    "non-JSON response: {e}; raw: {}",
                    String::from_utf8_lossy(&bytes).chars().take(200).collect::<String>()
                )))
            })?
        };

        // 5. Extract via JSONPath, if configured.
        let extracted = match &self.response_path {
            Some(path) => extract_path(&body, path)?,
            None => body,
        };
        Ok(extracted)
    }
}

fn extract_path(body: &Value, path: &str) -> NodeResult<Value> {
    if path == "$" {
        return Ok(body.clone());
    }
    let parsed = JsonPath::parse(path).map_err(|e| {
        NodeError::InvalidPayload(format!("invalid JSONPath `{path}`: {e}"))
    })?;
    let nodes: Vec<&Value> = parsed.query(body).all();
    match nodes.len() {
        0 => Err(NodeError::InvalidPayload(format!(
            "JSONPath `{path}` matched no nodes in response"
        ))),
        1 => Ok(nodes[0].clone()),
        _ => Ok(Value::Array(nodes.into_iter().cloned().collect())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::HttpBaseConfig;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_backend(base_url: String) -> Arc<HttpBackend> {
        Arc::new(
            HttpBackend::new(HttpBaseConfig {
                base_url,
                headers: HashMap::new(),
            })
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn get_with_url_template_and_response_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/forecast"))
            .and(query_param("q", "Lome"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "current_weather": {"temperature_c": 30.5}
            })))
            .mount(&server)
            .await;

        let backend = make_backend(server.uri());
        let spec = BindingSpec::Http {
            backend: "test".into(),
            url_template: "/forecast?q={{args.city}}".into(),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body_template: None,
            response_path: Some("$.current_weather".into()),
            timeout_ms: None,
        };
        let binding = HttpBinding::new(spec, backend).unwrap();
        let out = binding.invoke(json!({"city": "Lome"})).await.unwrap();
        assert_eq!(out, json!({"temperature_c": 30.5}));
    }

    #[tokio::test]
    async fn post_with_body_template() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"translation": "hello"})),
            )
            .mount(&server)
            .await;
        let backend = make_backend(server.uri());
        let spec = BindingSpec::Http {
            backend: "test".into(),
            url_template: "/translate".into(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            body_template: Some(json!({"text": "{{args.text}}", "target": "en"})),
            response_path: None,
            timeout_ms: None,
        };
        let binding = HttpBinding::new(spec, backend).unwrap();
        let out = binding.invoke(json!({"text": "bonjour"})).await.unwrap();
        assert_eq!(out, json!({"translation": "hello"}));
    }

    #[tokio::test]
    async fn upstream_error_bubbles_up_as_adapter_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/broken"))
            .respond_with(ResponseTemplate::new(500).set_body_string("kaboom"))
            .mount(&server)
            .await;
        let backend = make_backend(server.uri());
        let spec = BindingSpec::Http {
            backend: "test".into(),
            url_template: "/broken".into(),
            method: HttpMethod::Get,
            headers: HashMap::new(),
            body_template: None,
            response_path: None,
            timeout_ms: None,
        };
        let binding = HttpBinding::new(spec, backend).unwrap();
        let err = binding.invoke(json!({})).await.unwrap_err().to_string();
        assert!(err.contains("500"), "expected upstream 500 in error, got: {err}");
    }
}
