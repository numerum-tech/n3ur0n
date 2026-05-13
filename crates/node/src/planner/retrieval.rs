//! BM25 retrieval over the capability catalog.
//!
//! Why : the planner's compile prompt grows linearly with the catalog. Past
//! ~80 caps the LLM context saturates and quality drops (lost-in-the-middle,
//! tokenisation cost dominates). This module ranks tools against the user's
//! current message so the planner only sees the top-K most relevant remote
//! tools. Local tools always pass through (no point hiding caps the operator
//! explicitly configured).
//!
//! Implementation : Okapi BM25, standard parameters (k1=1.5, b=0.75). Hand-
//! rolled, no external index dependency. Tokeniser is intentionally simple:
//! lowercase, split on non-alphanumeric, drop tokens of length 1. That is
//! enough at this scale; embeddings + reranker are post-v0.2.

use std::collections::HashMap;

use crate::planner::catalog::ToolDef;

const BM25_K1: f32 = 1.5;
const BM25_B: f32 = 0.75;

/// One indexed document: the concatenated searchable text of a tool plus its
/// term-frequency table.
#[derive(Debug, Clone)]
struct DocumentIndex {
    /// term -> count in this document.
    term_freq: HashMap<String, u32>,
    /// Total number of tokens in the document.
    length: u32,
}

/// Precomputed corpus statistics for BM25 scoring.
#[derive(Debug, Clone)]
pub struct BM25Index {
    docs: Vec<DocumentIndex>,
    /// term -> number of documents containing the term.
    doc_freq: HashMap<String, u32>,
    avg_doc_length: f32,
    total_docs: u32,
}

impl BM25Index {
    /// Build an index over `tools`. Document N corresponds to `tools[N]`.
    pub fn build(tools: &[ToolDef]) -> Self {
        let mut docs = Vec::with_capacity(tools.len());
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        let mut total_length = 0u64;

        for t in tools {
            let text = searchable_text(t);
            let tokens = tokenise(&text);
            let mut term_freq: HashMap<String, u32> = HashMap::new();
            for tok in &tokens {
                *term_freq.entry(tok.clone()).or_insert(0) += 1;
            }
            for term in term_freq.keys() {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
            total_length += tokens.len() as u64;
            docs.push(DocumentIndex {
                term_freq,
                length: tokens.len() as u32,
            });
        }

        let total_docs = docs.len() as u32;
        let avg_doc_length = if total_docs > 0 {
            total_length as f32 / total_docs as f32
        } else {
            0.0
        };

        Self {
            docs,
            doc_freq,
            avg_doc_length,
            total_docs,
        }
    }

    /// BM25 score of `query` against document `doc_idx`.
    pub fn score(&self, query: &str, doc_idx: usize) -> f32 {
        let Some(doc) = self.docs.get(doc_idx) else {
            return 0.0;
        };
        if self.total_docs == 0 {
            return 0.0;
        }
        let tokens = tokenise(query);
        let mut score = 0.0f32;
        for term in &tokens {
            let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
            if df == 0.0 {
                continue;
            }
            let tf = *doc.term_freq.get(term).unwrap_or(&0) as f32;
            if tf == 0.0 {
                continue;
            }
            // Standard BM25 IDF (Robertson) with +0.5 smoothing.
            let idf = (((self.total_docs as f32 - df + 0.5) / (df + 0.5)) + 1.0).ln();
            let denom =
                tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (doc.length as f32 / self.avg_doc_length));
            let numer = tf * (BM25_K1 + 1.0);
            score += idf * (numer / denom);
        }
        score
    }
}

/// Concatenate the fields of a `ToolDef` we want indexed. Weight is implicit
/// (more important fields are repeated by the user's caller of `searchable_
/// text` if needed; for v0.2 a flat concat is fine).
fn searchable_text(t: &ToolDef) -> String {
    let cap = &t.cap;
    let mut s = String::new();
    s.push_str(&cap.name);
    s.push(' ');
    s.push_str(&cap.description);
    s.push(' ');
    for tag in &cap.tags {
        s.push_str(tag);
        s.push(' ');
    }
    if let Some(d) = &cap.disambiguation {
        s.push_str(d);
        s.push(' ');
    }
    if let Some(o) = &cap.output_semantic {
        s.push_str(o);
        s.push(' ');
    }
    // Examples' user intents are gold for retrieval: they are written in
    // user-language and align with how the planner's query will phrase
    // things.
    for ex in &cap.examples {
        s.push_str(&ex.user_intent);
        s.push(' ');
    }
    for ne in &cap.negative_examples {
        s.push_str(&ne.user_intent);
        s.push(' ');
    }
    s
}

fn tokenise(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.chars().count() > 1)
        .map(|s| s.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
    use serde_json::json;

    fn tool(name: &str, description: &str, tags: Vec<&str>, examples: Vec<&str>) -> ToolDef {
        ToolDef {
            peer_id: format!("n3:{name}peer"),
            peer_endpoint: Some(format!("http://{name}:4242")),
            cap: CapabilityDecl {
                name: name.into(),
                description: description.into(),
                schema_in: json!({}),
                schema_out: json!({}),
                mode: AccessMode::Free,
                pricing: None,
                tags: tags.into_iter().map(String::from).collect(),
                lobe_ids: vec![],
                examples: examples
                    .into_iter()
                    .map(|intent| CapabilityExample {
                        user_intent: intent.into(),
                        args: json!({}),
                        expected_output: json!({}),
                    })
                    .collect(),
                disambiguation: None,
                negative_examples: vec![],
                output_semantic: None,
                version: "0.0.0".into(),
                languages: vec![],
                countries: vec![],
            },
        }
    }

    #[test]
    fn tokenise_drops_short_tokens_and_lowercases() {
        let toks = tokenise("Hello World! Now is the moment a b cd");
        assert_eq!(toks, vec!["hello", "world", "now", "is", "the", "moment", "cd"]);
    }

    #[test]
    fn empty_corpus_score_zero() {
        let idx = BM25Index::build(&[]);
        assert_eq!(idx.score("anything", 0), 0.0);
    }

    #[test]
    fn relevant_tool_outranks_irrelevant() {
        let tools = vec![
            tool(
                "reverse",
                "Reverses a string character by character.",
                vec!["string", "transform"],
                vec!["reverse 'hello'"],
            ),
            tool(
                "random_int",
                "Returns a uniformly random integer in [min, max].",
                vec!["random", "number"],
                vec!["pick a random number between 1 and 10"],
            ),
            tool(
                "chat",
                "OpenAI-compatible chat completion via a large language model.",
                vec!["chat", "llm"],
                vec!["answer a free-form question"],
            ),
        ];
        let idx = BM25Index::build(&tools);

        // Reversing a string should rank reverse first.
        let q = "reverse the word hello";
        let scores: Vec<(usize, f32)> = (0..tools.len()).map(|i| (i, idx.score(q, i))).collect();
        let best = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(best.0, 0, "expected reverse to win; scores={scores:?}");

        // "random number" should rank random_int first.
        let q = "give me a random number";
        let scores: Vec<(usize, f32)> = (0..tools.len()).map(|i| (i, idx.score(q, i))).collect();
        let best = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(best.0, 1, "expected random_int to win; scores={scores:?}");

        // Free-form question matches chat best.
        let q = "answer this question please";
        let scores: Vec<(usize, f32)> = (0..tools.len()).map(|i| (i, idx.score(q, i))).collect();
        let best = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert_eq!(best.0, 2, "expected chat to win; scores={scores:?}");
    }

    #[test]
    fn unknown_terms_score_zero() {
        let tools = vec![tool("foo", "bar baz", vec![], vec!["do a foo"])];
        let idx = BM25Index::build(&tools);
        assert!(idx.score("zzzzzz nothingmatches", 0).abs() < 1e-6);
    }
}
