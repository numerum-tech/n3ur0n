//! Capability retrieval: BM25, optionally fused with embeddings.
//!
//! Scoring is split out from filtering on purpose. [`Retriever::score`]
//! may perform IO (embedding the query) and is therefore async;
//! `Catalog::filter_for_query` stays pure and synchronous so ranking
//! policy remains unit-testable without a network.
//!
//! # Why embeddings
//!
//! BM25 counts shared words. Measured against this project's own
//! catalog, that fails in two ways that matter:
//!
//! - `"Quelle heure est-il maintenant ?"` scores **0.000** on every
//!   English capability. The UI ships EN + FR; this is routine, not
//!   exotic.
//! - `"…then summarise what you find"` scores `summarize` at 0.384 while
//!   `"…summarize…"` scores it at 1.113. One letter of British/American
//!   spelling. Synonyms (`"condense this"`) fail the same way, and that
//!   one is monolingual — no amount of translation would fix it.
//!
//! Both are ranking failures, and today they are survivable because the
//! model still *sees* every capability. They become capability loss the
//! moment anything prunes on score, which is exactly why the relevance
//! floor was reverted (see the comment block in `catalog.rs`). Semantic
//! retrieval is the prerequisite for reconsidering it.
//!
//! # Degradation is mandatory, not best-effort
//!
//! Not every deployment has an embeddings endpoint: a node pointed at a
//! bare chat server has none, and a configured one can be down. Any
//! failure falls back to BM25-only — the behaviour that shipped before
//! this module existed. An embedding outage must never fail a dispatch,
//! and must never empty the catalog.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use n3ur0n_adapters::embeddings::{EmbeddingClient, cosine_similarity};
use tracing::{debug, warn};

use crate::planner::catalog::ToolDef;
use crate::planner::retrieval::{BM25Index, searchable_text};

/// Weight of the lexical (BM25) half of the fused score.
///
/// Lexical matching stays in the blend rather than being replaced:
/// embeddings are comparatively weak on exact, rare tokens — a
/// capability literally named `blob_ticket`, an ISO 4217 currency code,
/// a product name — which is precisely where BM25 is strongest. The
/// semantic half carries more weight because the failures we measured
/// (cross-language, synonyms, spelling variants) are all semantic.
const W_LEXICAL: f32 = 0.35;
/// Weight of the semantic (embedding) half. `W_LEXICAL + W_SEMANTIC == 1`.
const W_SEMANTIC: f32 = 0.65;

/// Scores capabilities against a user query.
#[derive(Debug)]
pub struct Retriever {
    /// `None` = BM25 only. That is a supported configuration, not a
    /// degraded one: it is what every node ran before embeddings existed.
    embeddings: Option<Arc<EmbeddingClient>>,
    /// Capability vectors, keyed by model + peer + cap + cap version.
    ///
    /// Embedding the catalog on every message would dominate dispatch
    /// latency, and capability text only changes when a publisher ships a
    /// new version — which `CapabilityDecl.version` already tracks, so it
    /// doubles as the invalidation key.
    cache: Mutex<HashMap<String, Vec<f32>>>,
}

impl Retriever {
    /// BM25-only retriever — the pre-embeddings behaviour.
    pub fn lexical() -> Self {
        Self {
            embeddings: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Hybrid retriever fusing BM25 with `client`'s embeddings.
    pub fn hybrid(client: Arc<EmbeddingClient>) -> Self {
        Self {
            embeddings: Some(client),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// True when embeddings are configured. Callers log this at startup
    /// so an operator can tell which retrieval path is live.
    pub fn is_hybrid(&self) -> bool {
        self.embeddings.is_some()
    }

    /// Score every tool against `query`, in `tools` order.
    ///
    /// Never fails: an embedding error degrades to the BM25 scores.
    pub async fn score(&self, tools: &[ToolDef], query: &str) -> Vec<f32> {
        let lexical = bm25_scores(tools, query);
        let Some(client) = &self.embeddings else {
            return lexical;
        };
        match self.semantic_scores(client, tools, query).await {
            Ok(semantic) => fuse(&lexical, &semantic),
            Err(e) => {
                // Deliberately a warning, not an error: the dispatch
                // continues on lexical scores exactly as it did before
                // embeddings were wired in.
                warn!(error = %e, "embedding retrieval failed; falling back to BM25 only");
                lexical
            }
        }
    }

    /// Cosine of each capability against the query, using the vector
    /// cache and embedding only what is missing.
    async fn semantic_scores(
        &self,
        client: &EmbeddingClient,
        tools: &[ToolDef],
        query: &str,
    ) -> Result<Vec<f32>, n3ur0n_adapters::AdapterError> {
        let keys: Vec<String> = tools.iter().map(|t| cache_key(client.model(), t)).collect();

        // Which capabilities still need a vector? Lock is taken and
        // released around the lookup — never held across an await.
        let missing: Vec<usize> = {
            let cache = self.cache.lock().expect("retriever cache poisoned");
            (0..tools.len())
                .filter(|&i| !cache.contains_key(&keys[i]))
                .collect()
        };

        // One request carries the query plus every missing capability, so
        // a cold catalog costs a single round trip rather than N+1.
        let mut inputs = Vec::with_capacity(missing.len() + 1);
        inputs.push(query.to_string());
        inputs.extend(missing.iter().map(|&i| searchable_text(&tools[i])));

        debug!(
            total = tools.len(),
            embedded = missing.len(),
            "embedding capabilities (cache miss count)"
        );
        let vectors = client.embed(&inputs).await?;
        let (query_vec, cap_vecs) = vectors.split_first().expect("at least the query");

        {
            let mut cache = self.cache.lock().expect("retriever cache poisoned");
            for (&i, v) in missing.iter().zip(cap_vecs.iter()) {
                cache.insert(keys[i].clone(), v.clone());
            }
        }

        let cache = self.cache.lock().expect("retriever cache poisoned");
        Ok(keys
            .iter()
            .map(|k| {
                cache
                    .get(k)
                    .map(|v| cosine_similarity(query_vec, v))
                    .unwrap_or(0.0)
            })
            .collect())
    }
}

/// Cache key. The model is part of it because vectors from different
/// models are not comparable; the cap version is what makes a publisher
/// updating its declaration invalidate the entry.
fn cache_key(model: &str, t: &ToolDef) -> String {
    format!("{model}|{}|{}|{}", t.peer_id, t.cap.name, t.cap.version)
}

fn bm25_scores(tools: &[ToolDef], query: &str) -> Vec<f32> {
    let index = BM25Index::build(tools);
    (0..tools.len()).map(|i| index.score(query, i)).collect()
}

/// Blend lexical and semantic scores.
///
/// BM25 is unbounded, so it is normalised against the best score in this
/// catalog before blending; cosine is already in `[0, 1]`. When BM25 has
/// no signal at all — every score zero, the cross-language case — the
/// lexical half contributes zero everywhere and ranking falls through to
/// the semantic half, which is the entire point.
fn fuse(lexical: &[f32], semantic: &[f32]) -> Vec<f32> {
    let top = lexical.iter().copied().fold(0.0f32, f32::max);
    lexical
        .iter()
        .zip(semantic.iter())
        .map(|(&l, &s)| {
            let l_norm = if top > 0.0 { l / top } else { 0.0 };
            W_LEXICAL * l_norm + W_SEMANTIC * s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
    use serde_json::json;

    fn tool(name: &str, desc: &str) -> ToolDef {
        ToolDef {
            peer_id: "n3:peer".into(),
            peer_endpoint: Some("http://x".into()),
            cap: CapabilityDecl {
                name: name.into(),
                description: desc.into(),
                schema_in: json!({}),
                schema_out: json!({}),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec![],
                lobe_ids: vec![],
                examples: vec![CapabilityExample {
                    user_intent: "x".into(),
                    args: json!({}),
                    expected_output: json!({}),
                }],
                disambiguation: None,
                negative_examples: vec![],
                output_semantic: None,
                version: "1.0.0".into(),
                languages: vec![],
                countries: vec![],
            },
        }
    }

    #[tokio::test]
    async fn lexical_only_matches_bm25() {
        let tools = vec![
            tool("reverse", "Reverses a string."),
            tool("time", "Returns the current server time."),
        ];
        let r = Retriever::lexical();
        assert!(!r.is_hybrid());
        let scores = r.score(&tools, "reverse this string").await;
        assert_eq!(scores, bm25_scores(&tools, "reverse this string"));
        assert!(scores[0] > scores[1], "matching cap ranks first");
    }

    /// The cross-language case. With no lexical signal the fused ranking
    /// must come entirely from the semantic half — this is what makes a
    /// French query find an English capability.
    #[test]
    fn fusion_falls_through_to_semantic_when_bm25_is_blind() {
        let lexical = vec![0.0, 0.0, 0.0];
        let semantic = vec![0.1, 0.9, 0.4];
        let fused = fuse(&lexical, &semantic);
        assert!(fused[1] > fused[2] && fused[2] > fused[0]);
        // Purely semantic ordering, scaled by the semantic weight.
        assert!((fused[1] - W_SEMANTIC * 0.9).abs() < 1e-6);
    }

    #[test]
    fn fusion_keeps_lexical_influence() {
        // Same semantic score, different lexical: lexical breaks the tie,
        // which is what keeps exact/rare token matching useful.
        let fused = fuse(&[10.0, 0.0], &[0.5, 0.5]);
        assert!(fused[0] > fused[1]);
        assert!((fused[0] - (W_LEXICAL + W_SEMANTIC * 0.5)).abs() < 1e-6);
    }

    #[test]
    fn weights_sum_to_one() {
        assert!((W_LEXICAL + W_SEMANTIC - 1.0).abs() < 1e-6);
    }

    /// End-to-end proof that hybrid retrieval buys something BM25 cannot,
    /// on the case that motivated it.
    ///
    /// Retrieval only *matters* when the catalog must be trimmed — with a
    /// catalog under the bound nothing is ever dropped and ranking is
    /// invisible. So this builds a catalog larger than the cut and asks,
    /// in French, for a capability declared in English. BM25 scores every
    /// capability 0.000 and the right one survives only by luck of catalog
    /// order; the fused score ranks it first.
    ///
    /// Ignored by default: needs an embeddings endpoint.
    /// `PLANNER_EVAL_EMBED_MODEL=bge-m3 cargo test -p n3ur0n-node --lib
    ///  retriever -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs an embeddings endpoint; run with --ignored"]
    async fn hybrid_finds_a_french_query_where_bm25_is_blind() {
        use n3ur0n_adapters::embeddings::{EmbeddingClient, EmbeddingConfig};

        let model = std::env::var("PLANNER_EVAL_EMBED_MODEL").unwrap_or_else(|_| "bge-m3".into());
        let base = std::env::var("PLANNER_EVAL_EMBED_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:11434".into());

        // Distractors first, so catalog order works *against* the answer:
        // if the right cap ends up on top it is because it was ranked
        // there, not because it happened to be listed early.
        let mut tools: Vec<ToolDef> = (0..20)
            .map(|i| {
                tool(
                    &format!("filler_{i}"),
                    "Performs an unrelated bookkeeping operation on internal records.",
                )
            })
            .collect();
        tools.push(tool("time", "Returns the current server time."));
        let target = tools.len() - 1;

        let query = "Quelle heure est-il maintenant ?";

        let lexical = bm25_scores(&tools, query);
        let lex_top = lexical.iter().copied().fold(0.0f32, f32::max);
        assert_eq!(
            lex_top, 0.0,
            "premise: BM25 has no signal at all for this query"
        );

        let client = EmbeddingClient::new(EmbeddingConfig {
            base_url: base,
            model,
            api_key: None,
        })
        .expect("embedding client");
        let scores = Retriever::hybrid(Arc::new(client))
            .score(&tools, query)
            .await;

        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("non-empty");
        assert_eq!(
            best, target,
            "hybrid must rank `time` first for a French query; scores={scores:?}"
        );
    }

    #[test]
    fn cache_key_tracks_model_and_version() {
        let t = tool("chat", "d");
        let a = cache_key("bge-m3", &t);
        let b = cache_key("other-model", &t);
        assert_ne!(a, b, "vectors from different models are not comparable");

        let mut newer = t.clone();
        newer.cap.version = "2.0.0".into();
        assert_ne!(
            cache_key("bge-m3", &t),
            cache_key("bge-m3", &newer),
            "a publisher shipping a new cap version must invalidate its vector"
        );
    }
}
