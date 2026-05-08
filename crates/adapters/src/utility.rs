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
use n3ur0n_core::capability::{AccessMode, CapabilityDecl};
use rand::Rng;
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
                let n: i64 = rand::thread_rng().gen_range(min..=max);
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
                description: "Returns the current server time (UTC, RFC 3339) and unix timestamp.".into(),
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
                tags: vec!["util".into(), "time".into()],
                lobe_ids: vec![],
            },
            CapabilityDecl {
                name: "random_int".into(),
                description: "Returns a random integer in [min, max] (inclusive).".into(),
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
                tags: vec!["util".into(), "random".into()],
                lobe_ids: vec![],
            },
            CapabilityDecl {
                name: "reverse".into(),
                description: "Reverses a string character by character.".into(),
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
                tags: vec!["util".into(), "string".into()],
                lobe_ids: vec![],
            },
            CapabilityDecl {
                name: "string_length".into(),
                description: "Counts characters and bytes in a string.".into(),
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
                tags: vec!["util".into(), "string".into()],
                lobe_ids: vec![],
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
