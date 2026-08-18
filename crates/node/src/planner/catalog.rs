//! Aggregated capability catalog (self + peers) for the planner.

use n3ur0n_core::capability::CapabilityDecl;
use n3ur0n_storage::{Db, peers};
use serde_json::Value;

use crate::error::NodeResult;
use crate::planner::retrieval::BM25Index;
use crate::registry::CapabilityRegistry;

/// Capability sourced from a specific peer (self or remote).
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub peer_id: String,
    pub peer_endpoint: Option<String>,
    pub cap: CapabilityDecl,
}

/// Aggregated read-only view of caps the planner can dispatch to.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub tools: Vec<ToolDef>,
}

/// Capability names that must never be advertised back to a planner — keeps
/// us from recursing plan→plan when v0.2 ships `PlanBackend`.
const EXCLUDED_CAP_NAMES: &[&str] = &["plan"];

/// Maximum number of *local* capabilities surfaced in the compile prompt.
///
/// Locals used to bypass ranking entirely and were unbounded: an operator
/// with a dozen skills put all twelve in front of the model on every
/// message, however irrelevant. Over-planning scales with the number of
/// visible-but-wrong skills, so they are now ranked and capped like
/// remotes. The bound is generous — locals are the operator's own,
/// deliberately configured caps, so they keep priority over remotes.
const LOCAL_TOP_K: usize = 12;

// ---------------------------------------------------------------------------
// Why there is no *score floor* here (measured 2026-07-30)
// ---------------------------------------------------------------------------
//
// The obvious next step is to drop capabilities scoring below some
// fraction of the best score, so the model cannot pick a tool it never
// sees. It was implemented, measured, and reverted. BM25 is lexical, and
// a floor turns that weakness from "ranked lower" (harmless — the model
// still sees the cap and picks it correctly) into "absent from the
// catalog" (silent capability loss). Two measurements killed it:
//
//   - "…then summarise what you find."  → `summarize` scores 0.384, rel 0.17
//     "…then summarize what you find."  → `summarize` scores 1.113, rel 0.49
//     A single letter (British vs American spelling) decides whether the
//     capability exists at all. On the eval suite this regressed the
//     `chain` category from 100% to 86%.
//
//   - "Quelle heure est-il maintenant ?" against an English catalog scores
//     *every* cap at 0.000. This project ships an EN/FR interface, so a
//     query the retriever has no opinion about is routine, not exotic.
//
// A floor only becomes safe once retrieval is semantic (embeddings, or
// hybrid BM25+embeddings) and a synonym or a translation still scores.
// Until then, ranking and a bound are safe — they only ever drop a cap
// when something else outranks it — while an absolute cut is not.

impl Catalog {
    /// Build a fresh catalog from local registry + cached peer descriptors.
    ///
    /// v0.2 contract: a `CapabilityDecl` MUST carry at least one example
    /// (`examples.len() >= 1`) to be included in the planner's catalog.
    /// Legacy v0.1 publishers (no `examples` field) are skipped with a
    /// warning so the planner never sees under-specified caps it cannot
    /// reliably invoke. Local caps are held to the same standard so the
    /// operator sees the warning during development.
    pub fn build(
        self_id: &str,
        local: &CapabilityRegistry,
        db: &Db,
        peer_limit: i64,
    ) -> NodeResult<Self> {
        let mut tools = Vec::new();
        // Local caps (no endpoint — invoked in-process via the local backend).
        for cap in local.all() {
            if EXCLUDED_CAP_NAMES.contains(&cap.name.as_str()) {
                continue;
            }
            if cap.examples.is_empty() {
                tracing::warn!(
                    cap = %cap.name,
                    "local capability has no examples; skipping from planner catalog \
                (v0.2 requires at least one CapabilityExample)"
                );
                continue;
            }
            tools.push(ToolDef {
                peer_id: self_id.to_string(),
                peer_endpoint: None,
                cap,
            });
        }
        // Remote caps from cached describe_self blobs.
        for record in peers::list(db, peer_limit)? {
            let Some(raw) = record.describe_self_cached.as_deref() else {
                continue;
            };
            let parsed: Value = match serde_json::from_str(raw) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let caps = parsed
                .get("capabilities")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            for c in caps {
                let decl: CapabilityDecl = match serde_json::from_value(c) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if EXCLUDED_CAP_NAMES.contains(&decl.name.as_str()) {
                    continue;
                }
                if decl.examples.is_empty() {
                    tracing::warn!(
                        peer = %record.id,
                        cap = %decl.name,
                        "remote capability has no examples; skipping from planner \
                    catalog (v0.2 requires at least one CapabilityExample)"
                    );
                    continue;
                }
                tools.push(ToolDef {
                    peer_id: record.id.clone(),
                    peer_endpoint: Some(record.endpoint.clone()),
                    cap: decl,
                });
            }
        }
        Ok(Self { tools })
    }

    /// Build a query-aware catalog: local caps always pass through, remote
    /// caps are scored against `user_query` via BM25 and the top
    /// `remote_top_k` survive. Tie-breaking is by original insertion order
    /// to keep results deterministic for tests + UI history.
    ///
    /// Why : compile prompts grow linearly with the catalog. Past ~80 caps
    /// the LLM context saturates. This filter caps the prompt size at a
    /// predictable bound regardless of network size.
    pub fn build_for_query(
        self_id: &str,
        local: &CapabilityRegistry,
        db: &Db,
        peer_limit: i64,
        user_query: &str,
        remote_top_k: usize,
    ) -> NodeResult<Self> {
        let full = Self::build(self_id, local, db, peer_limit)?;
        Ok(full.filter_for_query(user_query, remote_top_k))
    }

    /// Apply relevance filtering to an already-built catalog.
    ///
    /// Split out of [`build_for_query`](Self::build_for_query) so callers
    /// that assemble a catalog by other means — the planner accuracy
    /// suite, which builds one from fixtures — exercise the *same*
    /// filtering the runtime uses. Measuring the planner against an
    /// unfiltered catalog would report on a code path no user ever hits.
    ///
    /// `remote_top_k == 0` or an empty query disables filtering entirely,
    /// which is what tests and debug paths want.
    pub fn filter_for_query(self, user_query: &str, remote_top_k: usize) -> Self {
        if remote_top_k == 0 || user_query.trim().is_empty() {
            return self;
        }
        // Score *every* tool, local ones included — see LOCAL_TOP_K.
        let index = BM25Index::build(&self.tools);
        let scores: Vec<f32> = (0..self.tools.len())
            .map(|i| index.score(user_query, i))
            .collect();
        self.filter_with_scores(&scores, remote_top_k)
    }

    /// Rank and bound using scores computed elsewhere.
    ///
    /// Separated from scoring because scoring may do IO — the hybrid
    /// retriever embeds the query over HTTP — while this half is pure
    /// policy and stays synchronous and unit-testable. `scores` is
    /// positional: `scores[i]` belongs to `tools[i]`.
    ///
    /// A length mismatch means the caller paired the wrong scores with
    /// the wrong catalog, which would silently rank capabilities by
    /// another catalog's relevance. Filtering is skipped rather than
    /// applying a scrambled order.
    pub fn filter_with_scores(self, scores: &[f32], remote_top_k: usize) -> Self {
        if remote_top_k == 0 {
            return self;
        }
        if scores.len() != self.tools.len() {
            tracing::error!(
                scores = scores.len(),
                tools = self.tools.len(),
                "score/tool length mismatch; skipping catalog filtering"
            );
            return self;
        }

        let mut locals: Vec<(ToolDef, f32)> = Vec::new();
        let mut remotes: Vec<(ToolDef, f32)> = Vec::new();
        for (t, s) in self.tools.into_iter().zip(scores.iter().copied()) {
            if t.peer_endpoint.is_none() {
                locals.push((t, s));
            } else {
                remotes.push((t, s));
            }
        }

        // Rank each group only when it actually has to be trimmed.
        //
        // Reordering is not free: skill order in the compile prompt
        // perturbs the model's output. Sorting unconditionally regressed
        // an unrelated chain case (`reverse` emitted without its required
        // `text` arg) purely because the skills moved. So a group that
        // already fits its bound keeps catalog order, and sorting is paid
        // for only when something must be dropped.
        //
        // `sort_by` is stable, so equal scores keep catalog order — with
        // no lexical signal at all (every score 0.0, e.g. a French query
        // against English caps) this degrades to "first N in catalog
        // order" rather than dropping everything.
        if locals.len() > LOCAL_TOP_K {
            locals.sort_by(|a, b| b.1.total_cmp(&a.1));
        }
        if remotes.len() > remote_top_k {
            remotes.sort_by(|a, b| b.1.total_cmp(&a.1));
        }

        // Local-first ordering keeps prompts (and any upstream prefix
        // cache) stable across queries.
        let mut out: Vec<ToolDef> = locals
            .into_iter()
            .take(LOCAL_TOP_K)
            .map(|(t, _)| t)
            .collect();
        out.extend(remotes.into_iter().take(remote_top_k).map(|(t, _)| t));
        Self { tools: out }
    }

    /// Number of tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Tool name as advertised to the LLM: `<short_peer>::<cap>`. Caller
    /// is responsible for matching back to a `ToolDef` via [`find`].
    pub fn tool_name(&self, t: &ToolDef) -> String {
        let short = short_peer(&t.peer_id);
        format!("{short}::{}", t.cap.name)
    }

    /// The `(short_peer, capability)` pairs the planner may choose from,
    /// in catalog order (locals first, so the ordering is stable across
    /// dispatches).
    ///
    /// Feeds the constrained-decoding grammars, which enumerate these
    /// pairs so an out-of-catalog tool cannot be sampled at all. The
    /// peer component is the *short* form — the same one `tool_name` /
    /// [`find`](Self::find) and the compile prompt use — so a plan that
    /// satisfies the grammar resolves here by construction.
    pub fn tool_names(&self) -> Vec<(String, String)> {
        self.tools
            .iter()
            .map(|t| (short_peer(&t.peer_id), t.cap.name.clone()))
            .collect()
    }

    /// Resolve a tool name (`<short_peer>::<cap>`) back to its full
    /// `ToolDef`.
    pub fn find(&self, tool_name: &str) -> Option<&ToolDef> {
        let (peer, cap_name) = tool_name.split_once("::")?;

        self.tools
            .iter()
            .find(|t| short_peer(&t.peer_id) == peer && t.cap.name == cap_name)
    }

    /// Convert to OpenAI `tools` array.
    pub fn to_openai_tools(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                let name = self.tool_name(t);
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.cap.description,
                        "parameters": t.cap.schema_in,
                    }
                })
            })
            .collect()
    }
}

fn short_peer(peer_id: &str) -> String {
    // Drop the `n3:` prefix and keep the next 12 chars to keep tool names
    // short enough for LLMs but long enough to disambiguate.
    let trimmed = peer_id.strip_prefix("n3:").unwrap_or(peer_id);
    trimmed.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample};
    use n3ur0n_storage::{open_in_memory, peers::PeerRecord};
    use serde_json::json;

    fn cap(name: &str) -> CapabilityDecl {
        cap_with_examples(name, true)
    }

    fn cap_with_examples(name: &str, with_examples: bool) -> CapabilityDecl {
        CapabilityDecl {
            name: name.into(),
            description: format!("test {name}"),
            schema_in: json!({"type": "object"}),
            schema_out: json!({"type": "object"}),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec![],
            lobe_ids: vec![],
            examples: if with_examples {
                vec![CapabilityExample {
                    user_intent: format!("invoke {name}"),
                    args: json!({}),
                    expected_output: json!({}),
                }]
            } else {
                vec![]
            },
            disambiguation: None,
            negative_examples: vec![],
            output_semantic: None,
            version: "0.0.0".into(),
            languages: vec![],
            countries: vec![],
        }
    }

    #[test]
    fn builds_from_self_and_peers_excludes_plan() {
        let db = open_in_memory().unwrap();
        let registry = CapabilityRegistry::from_decls(vec![cap("chat"), cap("plan")]);

        let cached = serde_json::to_string(&json!({
            "instance_id": "n3:peera",
            "protocol_version": "n3ur0n/0.1.1",
            "updated_at": "2026-01-01T00:00:00Z",
            "capabilities": [
                {"name":"chat","description":"d","schema_in":{},"schema_out":{},"mode":"free","tags":[],"lobe_ids":[],"examples":[{"user_intent":"chat","args":{},"expected_output":{}}]},
                {"name":"plan","description":"d","schema_in":{},"schema_out":{},"mode":"free","tags":[],"lobe_ids":[],"examples":[{"user_intent":"plan","args":{},"expected_output":{}}]}
            ]
        })).unwrap();
        peers::upsert(
            &db,
            &PeerRecord {
                id: "n3:peera".into(),
                endpoint: "http://peera:4242".into(),
                alias: None,
                last_seen: Some(1),
                tls_fingerprint: None,
                describe_self_cached: Some(cached),
                describe_self_fetched_at: Some(1),
                source: None,
            },
        )
        .unwrap();

        let cat = Catalog::build("n3:selfaaa", &registry, &db, 100).unwrap();
        let names: Vec<&str> = cat.tools.iter().map(|t| t.cap.name.as_str()).collect();
        // Both `plan` entries (self + peer) excluded; both `chat` kept.
        assert_eq!(names.iter().filter(|&&n| n == "plan").count(), 0);
        assert_eq!(names.iter().filter(|&&n| n == "chat").count(), 2);
    }

    #[test]
    fn skips_caps_without_examples() {
        let db = open_in_memory().unwrap();
        let registry = CapabilityRegistry::from_decls(vec![
            cap_with_examples("good", true),
            cap_with_examples("bare", false),
        ]);

        // Remote cap with no examples — must also be dropped.
        let cached = serde_json::to_string(&json!({
            "instance_id": "n3:peera",
            "protocol_version": "n3ur0n/0.1.1",
            "updated_at": "2026-01-01T00:00:00Z",
            "capabilities": [
                {"name":"remote_good","description":"d","schema_in":{},"schema_out":{},"mode":"free","tags":[],"lobe_ids":[],"examples":[{"user_intent":"x","args":{},"expected_output":{}}]},
                {"name":"remote_bare","description":"d","schema_in":{},"schema_out":{},"mode":"free","tags":[],"lobe_ids":[]}
            ]
        })).unwrap();
        peers::upsert(
            &db,
            &PeerRecord {
                id: "n3:peera".into(),
                endpoint: "http://peera:4242".into(),
                alias: None,
                last_seen: Some(1),
                tls_fingerprint: None,
                describe_self_cached: Some(cached),
                describe_self_fetched_at: Some(1),
                source: None,
            },
        )
        .unwrap();

        let cat = Catalog::build("n3:selfaaa", &registry, &db, 100).unwrap();
        let names: Vec<&str> = cat.tools.iter().map(|t| t.cap.name.as_str()).collect();
        assert!(names.contains(&"good"));
        assert!(names.contains(&"remote_good"));
        assert!(!names.contains(&"bare"));
        assert!(!names.contains(&"remote_bare"));
    }

    #[test]
    fn build_for_query_keeps_locals_and_filters_remotes() {
        let db = open_in_memory().unwrap();
        // 1 local cap.
        let registry = CapabilityRegistry::from_decls(vec![cap("local_only")]);

        // Two remote peers with one cap each — only one matches the query.
        let peer_a = serde_json::to_string(&json!({
            "instance_id": "n3:peera",
            "protocol_version": "n3ur0n/0.1.1",
            "updated_at": "2026-01-01T00:00:00Z",
            "capabilities": [
                {"name":"weather","description":"Returns the weather forecast.","schema_in":{},"schema_out":{},"mode":"free","tags":["forecast","weather"],"lobe_ids":[],"examples":[{"user_intent":"what is the weather","args":{},"expected_output":{}}]}
            ]
        })).unwrap();
        let peer_b = serde_json::to_string(&json!({
            "instance_id": "n3:peerb",
            "protocol_version": "n3ur0n/0.1.1",
            "updated_at": "2026-01-01T00:00:00Z",
            "capabilities": [
                {"name":"translate","description":"Translates text between languages.","schema_in":{},"schema_out":{},"mode":"free","tags":["language","translation"],"lobe_ids":[],"examples":[{"user_intent":"translate to french","args":{},"expected_output":{}}]}
            ]
        })).unwrap();
        for (id, ep, raw) in [
            ("n3:peera", "http://peera:4242", peer_a),
            ("n3:peerb", "http://peerb:4242", peer_b),
        ] {
            peers::upsert(
                &db,
                &PeerRecord {
                    id: id.into(),
                    endpoint: ep.into(),
                    alias: None,
                    last_seen: Some(1),
                    tls_fingerprint: None,
                    describe_self_cached: Some(raw),
                    describe_self_fetched_at: Some(1),
                    source: None,
                },
            )
            .unwrap();
        }

        // top_k = 1 with a translation-flavoured query — translate should win.
        let cat = Catalog::build_for_query(
            "n3:selfaaa",
            &registry,
            &db,
            100,
            "translate this sentence into french",
            1,
        )
        .unwrap();
        let names: Vec<&str> = cat.tools.iter().map(|t| t.cap.name.as_str()).collect();
        assert!(names.contains(&"translate"), "matching remote kept");
        assert!(!names.contains(&"weather"), "irrelevant remote filtered");
        // Locals are ranked now, but only *bounded*, never cut by an
        // absolute relevance score — a single local is always under the
        // bound, so it survives regardless of how well it matches. See
        // the "no score floor" note above for why an absolute cut is
        // unsafe with a lexical retriever.
        assert!(names.contains(&"local_only"), "local under the bound kept");
    }

    /// Locals are ranked and bounded at `LOCAL_TOP_K`. Before this they
    /// bypassed ranking entirely and were unbounded, so an operator with
    /// many skills put every one of them in front of the model on every
    /// message — the visible-but-wrong skills that over-planning feeds on.
    #[test]
    fn locals_are_ranked_and_bounded() {
        let db = open_in_memory().unwrap();
        let mut decls: Vec<CapabilityDecl> = (0..LOCAL_TOP_K + 4)
            .map(|i| {
                let mut c = cap(&format!("filler_{i}"));
                c.description = format!("Unrelated filler capability number {i}.");
                c
            })
            .collect();
        let mut translate_local = cap("translate");
        translate_local.description = "Translates text between human languages.".into();
        decls.push(translate_local);
        let registry = CapabilityRegistry::from_decls(decls);

        let cat = Catalog::build_for_query(
            "n3:selfaaa",
            &registry,
            &db,
            100,
            "translate this sentence into french",
            5,
        )
        .unwrap();
        let names: Vec<&str> = cat.tools.iter().map(|t| t.cap.name.as_str()).collect();
        assert_eq!(
            cat.tools.len(),
            LOCAL_TOP_K,
            "locals bounded, got {names:?}"
        );
        assert!(
            names.contains(&"translate"),
            "the matching local must rank into the bound, got {names:?}"
        );
    }

    /// **The safety net.** BM25 is lexical, so a query in another language
    /// scores every capability at zero. That is the retriever having no
    /// opinion, not proof that no tool fits — pruning there would strip
    /// the catalog and lose `time` for a French speaker asking the time.
    /// This project ships an EN/FR UI, so it is a routine case.
    #[test]
    fn all_zero_scores_keep_the_whole_catalog() {
        let db = open_in_memory().unwrap();
        let mut time_cap = cap("time");
        time_cap.description = "Returns the current server time.".into();
        let mut rev = cap("reverse");
        rev.description = "Reverses a string.".into();
        let registry = CapabilityRegistry::from_decls(vec![time_cap, rev]);

        // No lexical overlap whatsoever with the English declarations.
        let cat = Catalog::build_for_query("n3:selfaaa", &registry, &db, 100, "quelle heure ?", 5)
            .unwrap();
        let names: Vec<&str> = cat.tools.iter().map(|t| t.cap.name.as_str()).collect();
        assert!(
            names.contains(&"time") && names.contains(&"reverse"),
            "no BM25 signal must not silently empty the catalog, got {names:?}"
        );
    }

    #[test]
    fn tool_name_round_trip() {
        let mut cat = Catalog::default();
        cat.tools.push(ToolDef {
            peer_id: "n3:abcdef1234567890".into(),
            peer_endpoint: Some("http://x".into()),
            cap: cap("chat"),
        });
        let name = cat.tool_name(&cat.tools[0]);
        assert_eq!(name, "abcdef123456::chat");
        let back = cat.find(&name).unwrap();
        assert_eq!(back.cap.name, "chat");
    }
}
