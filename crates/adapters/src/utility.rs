//! Utility backend: a deterministic backend exposing several small
//! capabilities. Useful for cluster smoke tests and to give the planner
//! something to choose between besides `chat` and `echo`.
//!
//! Capabilities :
//! - `time` — current server time (no input).
//! - `random_int` — random integer in a range.
//! - `reverse` — reverses a string.
//! - `string_length` — counts characters in a string.

use async_trait::async_trait;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl, CapabilityExample, NegativeExample};
use rand::RngExt;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{AdapterError, AdapterResult, Backend, HealthStatus};

#[derive(Debug, Default, Clone, Copy)]
pub struct UtilityBackend;

#[async_trait]
impl Backend for UtilityBackend {
    async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value> {
        match capability {
            "time" => Ok(json!({
                "now": OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .map_err(|e| AdapterError::Backend(e.to_string()))?,
                "unix": OffsetDateTime::now_utc().unix_timestamp()
            })),
            "random_int" => {
                let min = args.get("min").and_then(|v| v.as_i64()).unwrap_or(0);
                let max = args.get("max").and_then(|v| v.as_i64()).unwrap_or(100);
                if max < min {
                    return Err(AdapterError::Backend(
                        "max must be >= min".into(),
                    ));
                }
                let n: i64 = rand::rng().random_range(min..=max);
                Ok(json!({ "value": n, "min": min, "max": max }))
            }
            "reverse" => {
                let text = coerce_to_string(args.get("text"))
                    .ok_or_else(|| AdapterError::Backend("`text` required".into()))?;
                let reversed: String = text.chars().rev().collect();
                Ok(json!({ "reversed": reversed }))
            }
            "string_length" => {
                let text = coerce_to_string(args.get("text"))
                    .ok_or_else(|| AdapterError::Backend("`text` required".into()))?;
                Ok(json!({
                    "chars": text.chars().count(),
                    "bytes": text.len()
                }))
            }
            other => Err(AdapterError::UnknownCapability(other.to_string())),
        }
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        Ok(vec![
            CapabilityDecl {
                name: "time".into(),
                description: "Returns the current server time as RFC 3339 string and \
unix epoch seconds. Takes no arguments.".into(),
                schema_in: json!({"type": "object"}),
                schema_out: json!({
                    "type": "object",
                    "required": ["now", "unix"],
                    "properties": {
                        "now": {"type": "string"},
                        "unix": {"type": "integer"}
                    }
                }),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec!["util".into(), "time".into(), "clock".into()],
                lobe_ids: vec![],
                examples: vec![
                    CapabilityExample {
                        user_intent: "what time is it right now on the server?"
                            .into(),
                        args: json!({}),
                        expected_output: json!({
                            "now": "2026-05-11T12:34:56.789Z",
                            "unix": 1778492096
                        }),
                    },
                    CapabilityExample {
                        user_intent: "get a timestamp to use elsewhere in the plan"
                            .into(),
                        args: json!({}),
                        expected_output: json!({
                            "now": "2026-05-11T12:34:56.789Z",
                            "unix": 1778492096
                        }),
                    },
                ],
                disambiguation: Some(
                    "Server wall-clock only. Not a monotonic timer, not a date \
calculator, not a calendar — pick this when the user asks for the literal current \
moment."
                        .into(),
                ),
                negative_examples: vec![NegativeExample {
                    user_intent: "what day of the week is December 25 2030".into(),
                    why_not: "this cap returns *now*, not arbitrary dates; do not use \
for date arithmetic."
                        .into(),
                }],
                output_semantic: Some(
                    "Current UTC instant as both human-readable string and unix \
epoch seconds."
                        .into(),
                ),
                version: "0.1.0".into(),
                languages: vec![],
                countries: vec![],
            },
            CapabilityDecl {
                name: "random_int".into(),
                description: "Returns a uniformly random integer in [min, max] \
inclusive. Defaults: min=0, max=100.".into(),
                schema_in: json!({
                    "type": "object",
                    "properties": {
                        "min": {"type": "integer", "default": 0},
                        "max": {"type": "integer", "default": 100}
                    }
                }),
                schema_out: json!({
                    "type": "object",
                    "required": ["value", "min", "max"],
                    "properties": {
                        "value": {"type": "integer"},
                        "min": {"type": "integer"},
                        "max": {"type": "integer"}
                    }
                }),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec!["util".into(), "random".into(), "number".into()],
                lobe_ids: vec![],
                examples: vec![
                    CapabilityExample {
                        user_intent: "pick a random number between 1 and 10".into(),
                        args: json!({"min": 1, "max": 10}),
                        expected_output: json!({"value": 7, "min": 1, "max": 10}),
                    },
                    CapabilityExample {
                        user_intent: "give me a random integer".into(),
                        args: json!({}),
                        expected_output: json!({"value": 42, "min": 0, "max": 100}),
                    },
                ],
                disambiguation: Some(
                    "Integer randomness only. Each call is independent (no seed \
control). For random floats or weighted choice, this is the wrong cap."
                        .into(),
                ),
                negative_examples: vec![NegativeExample {
                    user_intent: "shuffle this list of items".into(),
                    why_not: "this cap returns one integer, not a permutation; do not \
use for list shuffling."
                        .into(),
                }],
                output_semantic: Some(
                    "A single uniformly-distributed integer drawn from [min, max]."
                        .into(),
                ),
                version: "0.1.0".into(),
                languages: vec![],
                countries: vec![],
            },
            CapabilityDecl {
                name: "reverse".into(),
                description: "Reverses a string character by character (Unicode \
scalar-value order). Input field is `text`.".into(),
                schema_in: json!({
                    "type": "object",
                    "required": ["text"],
                    "properties": {"text": {"type": "string"}}
                }),
                schema_out: json!({
                    "type": "object",
                    "required": ["reversed"],
                    "properties": {"reversed": {"type": "string"}}
                }),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec!["util".into(), "string".into(), "transform".into()],
                lobe_ids: vec![],
                examples: vec![
                    CapabilityExample {
                        user_intent: "reverse the letters in 'hello'".into(),
                        args: json!({"text": "hello"}),
                        expected_output: json!({"reversed": "olleh"}),
                    },
                    CapabilityExample {
                        user_intent: "spell this word backwards: bonjour".into(),
                        args: json!({"text": "bonjour"}),
                        expected_output: json!({"reversed": "ruojnob"}),
                    },
                ],
                disambiguation: Some(
                    "Character-level reversal only. Not for word-order reversal, not \
for translation, not for case flipping."
                        .into(),
                ),
                negative_examples: vec![
                    NegativeExample {
                        user_intent: "translate 'hello' to French".into(),
                        why_not: "translation is a chat-cap task; this cap only \
reverses characters."
                            .into(),
                    },
                    NegativeExample {
                        user_intent: "reverse the order of words in this sentence"
                            .into(),
                        why_not: "this reverses characters, producing a backwards \
string; word-order reversal is a different operation not exposed here."
                            .into(),
                    },
                ],
                output_semantic: Some(
                    "Input string with character order reversed."
                        .into(),
                ),
                version: "0.1.0".into(),
                languages: vec![],
                countries: vec![],
            },
            CapabilityDecl {
                name: "string_length".into(),
                description: "Counts characters (Unicode scalar values) and bytes \
(UTF-8) in a string. Input field is `text`.".into(),
                schema_in: json!({
                    "type": "object",
                    "required": ["text"],
                    "properties": {"text": {"type": "string"}}
                }),
                schema_out: json!({
                    "type": "object",
                    "required": ["chars", "bytes"],
                    "properties": {
                        "chars": {"type": "integer"},
                        "bytes": {"type": "integer"}
                    }
                }),
                mode: AccessMode::Free,
                pricing: None,
                tags: vec!["util".into(), "string".into(), "measure".into()],
                lobe_ids: vec![],
                examples: vec![
                    CapabilityExample {
                        user_intent: "how many characters in 'hello world'".into(),
                        args: json!({"text": "hello world"}),
                        expected_output: json!({"chars": 11, "bytes": 11}),
                    },
                    CapabilityExample {
                        user_intent: "byte size of 'héllo'".into(),
                        args: json!({"text": "héllo"}),
                        expected_output: json!({"chars": 5, "bytes": 6}),
                    },
                ],
                disambiguation: Some(
                    "Character count (Unicode scalars) and UTF-8 byte count only. \
Does not count words, lines, or graphemes."
                        .into(),
                ),
                negative_examples: vec![NegativeExample {
                    user_intent: "how many words in this sentence".into(),
                    why_not: "this cap counts characters, not whitespace-delimited \
words. Use a chat cap or a dedicated word-count cap if available."
                        .into(),
                }],
                output_semantic: Some(
                    "Character count and UTF-8 byte count of the input string."
                        .into(),
                ),
                version: "0.1.0".into(),
                languages: vec![],
                countries: vec![],
            },
        ])
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }
}

/// Coerce a JSON value to its string representation. Strings pass through;
/// numbers and booleans render via Display; null and missing values fail.
fn coerce_to_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn time_returns_now() {
        let b = UtilityBackend;
        let v = b.invoke("time", json!({})).await.unwrap();
        assert!(v["now"].is_string());
        assert!(v["unix"].is_i64());
    }

    #[tokio::test]
    async fn reverse_works() {
        let b = UtilityBackend;
        let v = b.invoke("reverse", json!({"text": "hello"})).await.unwrap();
        assert_eq!(v["reversed"], "olleh");
    }

    #[tokio::test]
    async fn random_int_in_range() {
        let b = UtilityBackend;
        for _ in 0..10 {
            let v = b
                .invoke("random_int", json!({"min": 1, "max": 10}))
                .await
                .unwrap();
            let n = v["value"].as_i64().unwrap();
            assert!((1..=10).contains(&n));
        }
    }

    #[tokio::test]
    async fn string_length_counts_chars_and_bytes() {
        let b = UtilityBackend;
        let v = b.invoke("string_length", json!({"text": "héllo"})).await.unwrap();
        assert_eq!(v["chars"], 5);
        assert_eq!(v["bytes"], 6); // é = 2 bytes UTF-8
    }

    #[tokio::test]
    async fn describe_lists_four_caps() {
        let decls = UtilityBackend.describe().await.unwrap();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["time", "random_int", "reverse", "string_length"]);
    }

    #[tokio::test]
    async fn reverse_coerces_number_to_string() {
        let b = UtilityBackend;
        let v = b.invoke("reverse", json!({"text": 740})).await.unwrap();
        assert_eq!(v["reversed"], "047");
    }
}
