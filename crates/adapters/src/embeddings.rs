//! Embedding client for an OpenAI-compatible `/v1/embeddings` endpoint.
//!
//! Speaks the same dialect as [`crate::openai`], so one deployment story
//! covers OpenAI itself, Ollama (`http://localhost:11434`), vLLM and
//! llama.cpp server. Ollama also has a native `/api/embed`, but going
//! through the OpenAI-compatible path keeps base-URL handling and
//! authentication identical to the chat backend.
//!
//! # Why this exists
//!
//! The planner's capability retrieval is BM25 — pure term overlap, with
//! no notion of meaning. Measured consequences on this project's own
//! catalog:
//!
//! - `"Quelle heure est-il maintenant ?"` scores **0.000** against every
//!   English capability. The UI ships EN + FR, so this is routine.
//! - `"…then summarise what you find"` scores the `summarize` cap at
//!   0.384 while `"…summarize…"` scores it at 1.113 — a single letter of
//!   British/American spelling.
//!
//! Embedding both sides and comparing vectors closes both gaps, and does
//! so far more cheaply than the obvious alternative of translating every
//! query: an embedding is a single forward pass, not a generation call.
//!
//! This client is deliberately *optional*. A node with no embedding
//! endpoint configured or reachable falls back to BM25 alone rather than
//! failing a dispatch — see `n3ur0n_node::planner::retriever`.

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::instrument;

use crate::openai::normalize_openai_base_url;
use crate::{AdapterError, AdapterResult};

/// Embedding calls are short; a generation-length timeout would mask a
/// hung endpoint for two minutes on a path the user is waiting on.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Static configuration for [`EmbeddingClient`].
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Base URL **without** the `/v1` suffix, same convention as
    /// [`crate::openai::OpenAIConfig`]. For Ollama:
    /// `http://localhost:11434`.
    pub base_url: String,
    /// Model identifier, e.g. `bge-m3`. Must be a *multilingual* model
    /// for the cross-language case above to work at all.
    pub model: String,
    /// Optional bearer token for hosted endpoints.
    pub api_key: Option<String>,
}

/// Client for one embedding endpoint.
#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    http: Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

impl EmbeddingClient {
    pub fn new(config: EmbeddingConfig) -> AdapterResult<Self> {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| AdapterError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: normalize_openai_base_url(&config.base_url),
            model: config.model,
            api_key: config.api_key,
        })
    }

    /// Model this client embeds with. Part of the vector cache key —
    /// vectors from different models are not comparable.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed a batch of texts, returning one vector per input **in input
    /// order**.
    ///
    /// Batching matters: a catalog refresh embeds every capability, and
    /// one request per cap would turn a cheap operation into a slow one.
    ///
    /// The response is re-ordered by the `index` field rather than
    /// trusted positionally — the OpenAI spec allows out-of-order data,
    /// and a silent mis-pairing here would attach each capability to
    /// another one's vector, which degrades ranking in a way that looks
    /// like a bad model rather than a bug.
    #[instrument(skip(self, texts), fields(model = %self.model, n = texts.len()))]
    pub async fn embed(&self, texts: &[String]) -> AdapterResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/v1/embeddings", self.base_url);
        let mut req = self
            .http
            .post(&url)
            .json(&json!({ "model": self.model, "input": texts }));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
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
                "embeddings endpoint returned {status}: {}",
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            )));
        }

        let parsed: EmbeddingResponse = serde_json::from_slice(&bytes)?;
        if parsed.data.len() != texts.len() {
            return Err(AdapterError::Backend(format!(
                "embeddings endpoint returned {} vectors for {} inputs",
                parsed.data.len(),
                texts.len()
            )));
        }

        let mut out = vec![Vec::new(); texts.len()];
        for datum in parsed.data {
            let slot = out.get_mut(datum.index).ok_or_else(|| {
                AdapterError::Backend(format!(
                    "embeddings endpoint returned out-of-range index {}",
                    datum.index
                ))
            })?;
            *slot = datum.embedding;
        }
        if let Some(pos) = out.iter().position(Vec::is_empty) {
            return Err(AdapterError::Backend(format!(
                "embeddings endpoint returned no vector for input {pos}"
            )));
        }
        Ok(out)
    }
}

/// Cosine similarity, clamped to `[0, 1]`.
///
/// Clamping matters for fusion: embedding similarities can go slightly
/// negative, and a negative term would let an unrelated capability drag
/// a *fused* score below one that has no signal at all. Zero-length
/// vectors score 0 rather than dividing by zero.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        // Orthogonal.
        assert!(cosine_similarity(&a, &[0.0, 1.0, 0.0]) < 1e-6);
        // Opposite vectors clamp to 0 rather than going negative, so they
        // can never drag a fused score below an unscored capability.
        assert_eq!(cosine_similarity(&a, &[-1.0, 0.0, 0.0]), 0.0);
        // Degenerate inputs are 0, not NaN or a panic.
        assert_eq!(cosine_similarity(&a, &[]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&a, &[0.0, 0.0, 0.0]), 0.0);
        // Length mismatch (two different embedding models) is 0, not a panic.
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn scale_invariant() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![10.0, 20.0, 30.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }
}
