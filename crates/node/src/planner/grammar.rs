//! Constrained-decoding grammars for the planner's compile step.
//!
//! Two representations of the same `Plan` shape:
//!
//! - [`plan_grammar()`] returns a llama.cpp / vLLM GBNF string. llama.cpp
//!   uses this directly via the `grammar` request field; vLLM with
//!   outlines also accepts GBNF. The grammar literally constrains token
//!   sampling so the model cannot emit a non-conforming character.
//!
//! - [`plan_json_schema()`] returns an OpenAI `response_format` payload
//!   (`{type: "json_schema", json_schema: {strict: true, schema: {...}}}`).
//!   Honoured by OpenAI ≥ 2024-08 and vLLM. Ollama 0.4 has partial
//!   `format: <schema>` support; we ship the schema there too via the
//!   `format` field (already in the args allowlist) as a best-effort.
//!
//! Both forbid extra top-level properties to keep small models from
//! padding the output with `description`, `reasoning`, etc.
//!
//! # Tool names are enumerated from the catalog (2026-07-29)
//!
//! Before this change both grammars were static and catalog-independent:
//! `peer` and `capability` were free strings (`{"type":"string"}` /
//! `step-peer ::= string`). Nothing structurally stopped the model from
//! emitting a tool that does not exist — hallucinated names were caught
//! after the fact by `validate_plan`, which kills the *whole* plan and
//! falls back to a no-tool answer. Selection correctness rested entirely
//! on prompt wording.
//!
//! Both grammars are now built per dispatch from the catalog's actual
//! `(peer, capability)` pairs, so an out-of-catalog tool is *unsampleable*
//! rather than merely rejected. The two representations differ in how
//! tightly they bind, because their expressiveness differs:
//!
//! | | binds |
//! |---|---|
//! | GBNF | the **pair** — `peer` and `capability` are emitted as one alternation branch, so a valid peer cannot be married to a capability it does not expose |
//! | JSON Schema | each field **independently** (`enum` per field). A cross-paired step is still sampleable; `validate_plan` catches it via `catalog.find()` |
//!
//! Per-field enums are used for JSON Schema because OpenAI strict mode
//! constrains `anyOf` usage and nesting depth; the pair-exact encoding
//! would need one sub-schema per tool. The looser form is still strictly
//! better than a free string, and the executor's validation already
//! covers the residual case.
//!
//! Which binding a given deployment actually gets depends on the upstream,
//! since all three fields are forwarded and unknown ones are ignored
//! (`CHAT_ARG_ALLOWLIST` in `n3ur0n-adapters`):
//!
//! | upstream | honours | binding strength |
//! |---|---|---|
//! | llama.cpp | `grammar` (GBNF) | pair-exact |
//! | Ollama | `format` (JSON Schema) | per-field enum |
//! | OpenAI ≥ 2024-08, vLLM | `response_format` | per-field enum |
//!
//! So the common Ollama deployment gets per-field enums, and
//! `validate_plan`'s `catalog.find()` remains load-bearing for the
//! cross-paired case. Do not remove it on the assumption that the
//! grammar already covers it.
//!
//! `args` stays unconstrained in both: per-cap schemas are validated
//! separately in `validate_plan`, and re-emitting them every dispatch
//! would blow up the grammar. Only the *names* are enumerated, which
//! costs a few hundred bytes.
//!
//! With an empty catalog there is nothing to enumerate — an empty GBNF
//! alternation and an empty JSON `enum` are both invalid — so both fall
//! back to the free-string form. That path only ever produces
//! `{"plan": []}`, which is the correct answer when no tool exists.

use serde_json::{Value, json};

/// One selectable tool: `(short_peer, capability)`, exactly as the
/// compile prompt presents it and as `Catalog::find` resolves it.
pub type ToolName = (String, String);

/// llama.cpp-flavoured GBNF grammar for the `Plan` schema, with `peer` +
/// `capability` enumerated as pairs from `tools`. Strict: no extra
/// fields, no tool outside the catalog.
///
/// Falls back to the free-string grammar when `tools` is empty.
///
/// Keep this in sync with the `Plan` struct in `crate::planner::plan`.
pub fn plan_grammar(tools: &[ToolName]) -> String {
    let deduped = dedup(tools);
    if deduped.is_empty() {
        return FREE_PLAN_GBNF.to_string();
    }

    // One alternation branch per tool, each pinning `peer` and
    // `capability` together so they cannot be mixed across tools.
    let branches = deduped
        .iter()
        .enumerate()
        .map(|(i, (peer, cap))| {
            format!(
                "tool-{i}      ::= \"\\\"peer\\\"\" ws \":\" ws {} ws \",\" ws \
                 \"\\\"capability\\\"\" ws \":\" ws {}",
                gbnf_json_string(peer),
                gbnf_json_string(cap),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let alternatives = (0..deduped.len())
        .map(|i| format!("tool-{i}"))
        .collect::<Vec<_>>()
        .join(" | ");

    format!("{PLAN_GBNF_HEAD}step-tool   ::= {alternatives}\n{branches}\n{PLAN_GBNF_TAIL}")
}

/// OpenAI-style `response_format` object usable as the `response_format`
/// argument to a `chat` cap. Strict mode (`additionalProperties: false`).
pub fn plan_response_format(tools: &[ToolName]) -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "n3ur0n_plan",
            "strict": true,
            "schema": plan_json_schema(tools)
        }
    })
}

/// Bare JSON Schema document describing a `Plan`. Useful on backends that
/// accept a schema directly (Ollama 0.4+ via `format`).
///
/// `peer` and `capability` carry an `enum` of the catalog's values when
/// `tools` is non-empty; see the module docs for why the binding is
/// per-field rather than per-pair here.
pub fn plan_json_schema(tools: &[ToolName]) -> Value {
    let deduped = dedup(tools);

    let (peer_schema, cap_schema) = if deduped.is_empty() {
        (
            json!({ "type": "string", "minLength": 1 }),
            json!({ "type": "string", "minLength": 1 }),
        )
    } else {
        // `deduped` is unique by *pair*, so each component still repeats
        // whenever a peer exposes several caps (or several peers expose
        // the same cap name). `Vec::dedup` only drops *consecutive*
        // duplicates and would leave those in, so dedup by set while
        // preserving catalog order — stable ordering keeps the prompt
        // (and any upstream prefix cache) stable across dispatches.
        (
            json!({ "type": "string", "enum": unique_in_order(deduped.iter().map(|(p, _)| p)) }),
            json!({ "type": "string", "enum": unique_in_order(deduped.iter().map(|(_, c)| c)) }),
        )
    };

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["plan"],
        "properties": {
            "plan": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "peer", "capability", "args"],
                    "properties": {
                        "id":          { "type": "string", "minLength": 1 },
                        "peer":        peer_schema,
                        "capability":  cap_schema,
                        "args":        { "type": "object" },
                        "depends_on":  {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            }
        }
    })
}

/// Collect `items` into a `Vec`, dropping repeats anywhere in the
/// sequence (not just consecutive ones) and keeping first-seen order.
fn unique_in_order<'a, I: Iterator<Item = &'a String>>(items: I) -> Vec<&'a str> {
    let mut seen = std::collections::HashSet::new();
    items
        .filter(|s| seen.insert(s.as_str()))
        .map(String::as_str)
        .collect()
}

/// Drop duplicate pairs while preserving catalog order (which is
/// local-caps-first, so the ordering stays stable across dispatches).
fn dedup(tools: &[ToolName]) -> Vec<ToolName> {
    let mut seen = std::collections::HashSet::new();
    tools
        .iter()
        .filter(|t| seen.insert((t.0.clone(), t.1.clone())))
        .cloned()
        .collect()
}

/// Render `s` as a GBNF terminal matching the JSON string `"s"` —
/// i.e. the emitted text includes the surrounding JSON quotes.
///
/// **Two escaping layers stack here and both are required.** The model
/// emits JSON, so a `"` inside `s` reaches the wire as `\"` (JSON
/// escape). GBNF terminals are themselves double-quoted, so that
/// backslash and that quote each need a further GBNF escape — a name
/// containing `"` ends up as `\\\"` in the grammar text. Escaping only
/// one layer lets a stray quote terminate the terminal early and
/// corrupts every rule after it.
///
/// Peer ids are Base32 (never affected); capability names come from
/// operator-written manifests, so this is defensive rather than routine.
fn gbnf_json_string(s: &str) -> String {
    // Layer 1 — JSON: produces the quoted, JSON-escaped form of `s`.
    let json = serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""));
    // Layer 2 — GBNF: escape backslashes first, then quotes, so the
    // backslashes introduced here are not themselves re-escaped.
    let gbnf = json.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{gbnf}\"")
}

/// Head of the generated grammar: everything up to (but excluding) the
/// `step-tool` rule, which is emitted per dispatch.
const PLAN_GBNF_HEAD: &str = r#"root        ::= ws "{" ws "\"plan\"" ws ":" ws "[" ws steps? ws "]" ws "}" ws
steps       ::= step (ws "," ws step)*
step        ::= "{" ws step-fields ws "}"
step-fields ::= step-id ws "," ws step-tool ws "," ws step-args (ws "," ws step-deps)?
step-id     ::= "\"id\""         ws ":" ws string
step-args   ::= "\"args\""       ws ":" ws object
step-deps   ::= "\"depends_on\"" ws ":" ws "[" ws (string (ws "," ws string)*)? ws "]"
"#;

/// Tail of the generated grammar: the shared JSON value rules.
const PLAN_GBNF_TAIL: &str = r#"
value       ::= object | array | string | number | "true" | "false" | "null"
object      ::= "{" ws (pair (ws "," ws pair)*)? ws "}"
pair        ::= string ws ":" ws value
array       ::= "[" ws (value (ws "," ws value)*)? ws "]"
string      ::= "\"" char* "\""
char        ::= [^"\\\x00-\x1F] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex         ::= [0-9a-fA-F]
number      ::= "-"? int frac? exp?
int         ::= "0" | [1-9] [0-9]*
frac        ::= "." [0-9]+
exp         ::= [eE] [-+]? [0-9]+
ws          ::= [ \t\n\r]*
"#;

/// Fallback grammar used when the catalog is empty: `peer` / `capability`
/// stay free strings because there is nothing to enumerate. The only
/// correct output on an empty catalog is `{"plan": []}`.
const FREE_PLAN_GBNF: &str = r#"root        ::= ws "{" ws "\"plan\"" ws ":" ws "[" ws steps? ws "]" ws "}" ws
steps       ::= step (ws "," ws step)*
step        ::= "{" ws step-fields ws "}"
step-fields ::= step-id ws "," ws step-peer ws "," ws step-cap ws "," ws step-args (ws "," ws step-deps)?
step-id     ::= "\"id\""         ws ":" ws string
step-peer   ::= "\"peer\""       ws ":" ws string
step-cap    ::= "\"capability\"" ws ":" ws string
step-args   ::= "\"args\""       ws ":" ws object
step-deps   ::= "\"depends_on\"" ws ":" ws "[" ws (string (ws "," ws string)*)? ws "]"

value       ::= object | array | string | number | "true" | "false" | "null"
object      ::= "{" ws (pair (ws "," ws pair)*)? ws "}"
pair        ::= string ws ":" ws value
array       ::= "[" ws (value (ws "," ws value)*)? ws "]"
string      ::= "\"" char* "\""
char        ::= [^"\\\x00-\x1F] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex         ::= [0-9a-fA-F]
number      ::= "-"? int frac? exp?
int         ::= "0" | [1-9] [0-9]*
frac        ::= "." [0-9]+
exp         ::= [eE] [-+]? [0-9]+
ws          ::= [ \t\n\r]*
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<ToolName> {
        vec![
            ("abc123def456".into(), "chat".into()),
            ("xyz789ghi012".into(), "translate".into()),
        ]
    }

    #[test]
    fn grammar_has_root_rule_first() {
        for g in [plan_grammar(&tools()), plan_grammar(&[])] {
            let first_non_blank = g
                .lines()
                .find(|l| !l.trim().is_empty())
                .expect("grammar is non-empty");
            assert!(
                first_non_blank.starts_with("root"),
                "first rule must be `root`, got: {first_non_blank}"
            );
        }
    }

    #[test]
    fn grammar_mentions_all_step_fields() {
        let g = plan_grammar(&tools());
        // GBNF literals contain backslash-escaped quotes around field names:
        // e.g. `"\"id\""` in source ⇒ the GBNF text `"\"id\""`.
        for field in [
            "\\\"id\\\"",
            "\\\"peer\\\"",
            "\\\"capability\\\"",
            "\\\"args\\\"",
            "\\\"depends_on\\\"",
        ] {
            assert!(g.contains(field), "grammar missing {field}");
        }
    }

    #[test]
    fn grammar_enumerates_catalog_pairs() {
        let g = plan_grammar(&tools());
        // Each tool becomes its own alternation branch pinning peer+cap.
        assert!(g.contains("step-tool   ::= tool-0 | tool-1"));
        assert!(g.contains("\\\"abc123def456\\\""));
        assert!(g.contains("\\\"chat\\\""));
        assert!(g.contains("\\\"xyz789ghi012\\\""));
        assert!(g.contains("\\\"translate\\\""));
        // The free-string rules must be gone: that was the hole.
        assert!(
            !g.lines().any(|l| l.starts_with("step-peer")),
            "free-string peer rule must not survive enumeration"
        );
        assert!(
            !g.lines().any(|l| l.starts_with("step-cap ")),
            "free-string capability rule must not survive enumeration"
        );
    }

    #[test]
    fn grammar_pins_peer_and_capability_together() {
        let g = plan_grammar(&tools());
        // `abc123def456` may only ever be followed by `chat`, never by
        // `translate` — that is the cross-pairing hole a per-field enum
        // would leave open.
        let branch = g
            .lines()
            .find(|l| l.contains("\\\"abc123def456\\\""))
            .expect("branch for first tool");
        assert!(branch.contains("\\\"chat\\\""));
        assert!(!branch.contains("\\\"translate\\\""));
    }

    /// Collect `name` for every `name ::= ...` rule.
    fn defined_rules(g: &str) -> std::collections::HashSet<String> {
        g.lines()
            .filter_map(|l| l.split_once("::="))
            .map(|(head, _)| head.trim().to_string())
            .collect()
    }

    /// Collect every bare identifier appearing on a right-hand side,
    /// ignoring anything inside a `"..."` terminal or a `[...]` class.
    fn referenced_rules(g: &str) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for line in g.lines() {
            let Some((_, rhs)) = line.split_once("::=") else {
                continue;
            };
            let mut chars = rhs.chars().peekable();
            let mut ident = String::new();
            while let Some(c) = chars.next() {
                match c {
                    // Skip a terminal, honouring backslash escapes.
                    '"' => {
                        while let Some(t) = chars.next() {
                            if t == '\\' {
                                chars.next();
                            } else if t == '"' {
                                break;
                            }
                        }
                    }
                    // Skip a character class.
                    '[' => {
                        while let Some(t) = chars.next() {
                            if t == '\\' {
                                chars.next();
                            } else if t == ']' {
                                break;
                            }
                        }
                    }
                    c if c.is_alphanumeric() || c == '-' || c == '_' => ident.push(c),
                    _ => {
                        if !ident.is_empty() {
                            out.insert(std::mem::take(&mut ident));
                        }
                    }
                }
            }
            if !ident.is_empty() {
                out.insert(ident);
            }
        }
        out
    }

    /// Structural validity: substring assertions prove the *content* is
    /// there, not that the grammar parses. A rule referenced but never
    /// defined is the likeliest generation bug (e.g. the `step-tool`
    /// alternation naming a `tool-N` that was never emitted) and
    /// llama.cpp would only reject it at dispatch time, in production.
    #[test]
    fn every_referenced_rule_is_defined() {
        for (label, g) in [
            ("enumerated", plan_grammar(&tools())),
            ("single-tool", plan_grammar(&tools()[..1])),
            ("empty-fallback", plan_grammar(&[])),
        ] {
            let defined = defined_rules(&g);
            let missing: Vec<_> = referenced_rules(&g)
                .into_iter()
                .filter(|r| !defined.contains(r))
                .collect();
            assert!(
                missing.is_empty(),
                "{label}: undefined rules referenced: {missing:?}\n{g}"
            );
            assert!(defined.contains("root"), "{label}: no root rule");
        }
    }

    /// Every emitted `tool-N` branch must be reachable from `step-tool`,
    /// and vice versa — a mismatch means some catalog entry is either
    /// unselectable or dangling.
    #[test]
    fn tool_branches_match_the_alternation() {
        let t = tools();
        let g = plan_grammar(&t);
        let alternation = g
            .lines()
            .find(|l| l.starts_with("step-tool"))
            .expect("step-tool rule");
        for i in 0..t.len() {
            let name = format!("tool-{i}");
            assert!(
                alternation.contains(&name),
                "{name} missing from alternation: {alternation}"
            );
            assert!(
                g.lines().any(|l| l.starts_with(&format!("{name} "))),
                "{name} referenced but never defined"
            );
        }
        assert!(
            !alternation.contains(&format!("tool-{}", t.len())),
            "alternation references more branches than there are tools"
        );
    }

    #[test]
    fn grammar_deduplicates_repeated_pairs() {
        let mut t = tools();
        t.push(("abc123def456".into(), "chat".into()));
        let g = plan_grammar(&t);
        assert!(g.contains("step-tool   ::= tool-0 | tool-1\n"));
        assert!(!g.contains("tool-2"));
    }

    #[test]
    fn grammar_escapes_quotes_in_names() {
        let g = plan_grammar(&[("peer".into(), "we\"ird".into())]);
        // `we"ird` serialises to JSON as `we\"ird`; the GBNF layer then
        // escapes both that backslash and that quote, so the terminal
        // carries `\\\"`. Escaping only one layer would let the quote
        // close the terminal early and corrupt every following rule.
        assert!(g.contains(r#"we\\\"ird"#), "got: {g}");
        // Sanity: the branch is still a single well-formed line.
        let branch = g
            .lines()
            .find(|l| l.starts_with("tool-0"))
            .expect("branch emitted");
        assert!(branch.contains("capability"), "got: {branch}");
    }

    #[test]
    fn empty_catalog_falls_back_to_free_strings() {
        let g = plan_grammar(&[]);
        // Free-string rules are back (exact inner spacing is cosmetic, so
        // match on the rule head and its tail separately).
        let peer_rule = g
            .lines()
            .find(|l| l.starts_with("step-peer"))
            .expect("free-string peer rule");
        assert!(peer_rule.ends_with("ws string"), "got: {peer_rule}");
        let cap_rule = g
            .lines()
            .find(|l| l.starts_with("step-cap"))
            .expect("free-string capability rule");
        assert!(cap_rule.ends_with("ws string"), "got: {cap_rule}");
        assert!(!g.contains("tool-0"));
    }

    #[test]
    fn json_schema_validates_minimal_plan() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema(&tools());
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let valid = json!({
            "plan": [
                {"id": "s1", "peer": "abc123def456", "capability": "chat", "args": {}}
            ]
        });
        assert!(compiled.is_valid(&valid));
        let empty = json!({"plan": []});
        assert!(compiled.is_valid(&empty));
    }

    #[test]
    fn json_schema_rejects_tool_outside_catalog() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema(&tools());
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let bad_peer = json!({
            "plan": [{"id": "s1", "peer": "hallucinated", "capability": "chat", "args": {}}]
        });
        assert!(
            !compiled.is_valid(&bad_peer),
            "unknown peer must be rejected"
        );
        let bad_cap = json!({
            "plan": [{"id": "s1", "peer": "abc123def456", "capability": "nope", "args": {}}]
        });
        assert!(
            !compiled.is_valid(&bad_cap),
            "unknown capability must be rejected"
        );
    }

    #[test]
    fn json_schema_without_catalog_keeps_free_strings() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema(&[]);
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        // Nothing to enumerate: any name parses, and the empty plan (the
        // only correct answer here) stays valid.
        assert!(compiled.is_valid(&json!({"plan": []})));
        assert!(compiled.is_valid(&json!({
            "plan": [{"id": "s1", "peer": "whatever", "capability": "x", "args": {}}]
        })));
    }

    /// A peer exposing several caps (and a cap name shared by several
    /// peers) must appear exactly once in each enum. `Vec::dedup` alone
    /// only drops *consecutive* repeats and would leave duplicates here.
    #[test]
    fn json_schema_enums_have_no_duplicates() {
        let t: Vec<ToolName> = vec![
            ("peer_a".into(), "chat".into()),
            ("peer_b".into(), "chat".into()),
            ("peer_a".into(), "time".into()),
        ];
        let schema = plan_json_schema(&t);
        let props = &schema["properties"]["plan"]["items"]["properties"];

        let peers: Vec<&str> = props["peer"]["enum"]
            .as_array()
            .expect("peer enum")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(peers, vec!["peer_a", "peer_b"], "peers deduped, in order");

        let caps: Vec<&str> = props["capability"]["enum"]
            .as_array()
            .expect("capability enum")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(caps, vec!["chat", "time"], "caps deduped, in order");
    }

    #[test]
    fn json_schema_rejects_extra_top_level_field() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema(&tools());
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let extra = json!({
            "plan": [],
            "reasoning": "I think we should..."
        });
        assert!(!compiled.is_valid(&extra));
    }

    #[test]
    fn json_schema_rejects_step_missing_required_field() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema(&tools());
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let bad = json!({
            "plan": [
                {"id": "s1", "peer": "abc123def456"}
            ]
        });
        assert!(!compiled.is_valid(&bad));
    }

    #[test]
    fn response_format_wraps_schema() {
        let rf = plan_response_format(&tools());
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["strict"], true);
        assert!(rf["json_schema"]["schema"]["properties"]["plan"].is_object());
    }
}
