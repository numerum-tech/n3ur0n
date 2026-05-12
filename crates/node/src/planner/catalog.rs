//! Aggregated capability catalog (self + peers) for the planner.

use n3ur0n_core::capability::CapabilityDecl;
use n3ur0n_storage::{peers, Db};
use serde_json::Value;

use crate::error::NodeResult;
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

    /// Resolve a tool name (`<short_peer>::<cap>`) back to its full
    /// `ToolDef`.
    pub fn find(&self, tool_name: &str) -> Option<&ToolDef> {
        let mut split = tool_name.splitn(2, "::");
        let peer = split.next()?;
        let cap_name = split.next()?;
        self.tools.iter().find(|t| {
            short_peer(&t.peer_id) == peer && t.cap.name == cap_name
        })
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
    use n3ur0n_core::capability::{
        AccessMode, CapabilityDecl, CapabilityExample,
    };
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
