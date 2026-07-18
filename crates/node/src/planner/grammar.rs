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

use serde_json::{Value, json};

/// llama.cpp-flavoured GBNF grammar for the `Plan` schema. Strict: no
/// extra fields, no trailing whitespace beyond what the grammar admits.
///
/// Keep this in sync with the `Plan` struct in `crate::planner::plan`.
pub fn plan_grammar() -> &'static str {
    PLAN_GBNF
}

/// OpenAI-style `response_format` object usable as the `response_format`
/// argument to a `chat` cap. Strict mode (`additionalProperties: false`).
pub fn plan_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "n3ur0n_plan",
            "strict": true,
            "schema": plan_json_schema()
        }
    })
}

/// Bare JSON Schema document describing a `Plan`. Useful on backends that
/// accept a schema directly (Ollama 0.4+ via `format`).
pub fn plan_json_schema() -> Value {
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
                        "peer":        { "type": "string", "minLength": 1 },
                        "capability":  { "type": "string", "minLength": 1 },
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

/// GBNF grammar (llama.cpp dialect). Order of rules matters: the root
/// rule must come first.
///
/// We accept any JSON value inside `args` (the per-cap schema validates
/// args separately in `validate_plan`); this keeps the grammar compact
/// and avoids re-emitting per-cap schemas every dispatch.
const PLAN_GBNF: &str = r#"root        ::= ws "{" ws "\"plan\"" ws ":" ws "[" ws steps? ws "]" ws "}" ws
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

    #[test]
    fn grammar_has_root_rule_first() {
        let g = plan_grammar();
        let first_non_blank = g
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("grammar is non-empty");
        assert!(
            first_non_blank.starts_with("root"),
            "first rule must be `root`, got: {first_non_blank}"
        );
    }

    #[test]
    fn grammar_mentions_all_step_fields() {
        let g = plan_grammar();
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
    fn json_schema_validates_minimal_plan() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema();
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let valid = json!({
            "plan": [
                {"id": "s1", "peer": "abc", "capability": "chat", "args": {}}
            ]
        });
        assert!(compiled.is_valid(&valid));
        let empty = json!({"plan": []});
        assert!(compiled.is_valid(&empty));
    }

    #[test]
    fn json_schema_rejects_extra_top_level_field() {
        use jsonschema::JSONSchema;
        let schema = plan_json_schema();
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
        let schema = plan_json_schema();
        let compiled = JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .unwrap();
        let bad = json!({
            "plan": [
                {"id": "s1", "peer": "abc"}
            ]
        });
        assert!(!compiled.is_valid(&bad));
    }

    #[test]
    fn response_format_wraps_schema() {
        let rf = plan_response_format();
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["strict"], true);
        assert!(rf["json_schema"]["schema"]["properties"]["plan"].is_object());
    }
}
