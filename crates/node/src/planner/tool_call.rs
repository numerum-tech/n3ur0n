//! Helpers for parsing/building OpenAI tool_calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Shape of a single tool_call as emitted by an OpenAI-compatible LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// LLMs send `arguments` as a JSON-encoded **string**.
    pub arguments: String,
}

impl ToolCall {
    /// Parse the `arguments` field as JSON. Empty string yields `{}`.
    pub fn parsed_args(&self) -> Result<Value, serde_json::Error> {
        let raw = self.function.arguments.trim();
        if raw.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        serde_json::from_str(raw)
    }
}

/// Try to extract `tool_calls` from a chat completion response.
pub fn extract_tool_calls(message: &Value) -> Vec<ToolCall> {
    message
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| serde_json::from_value::<ToolCall>(c.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_arguments_string() {
        let tc = ToolCall {
            id: "call_1".into(),
            kind: Some("function".into()),
            function: ToolCallFunction {
                name: "x::y".into(),
                arguments: r#"{"a":1,"b":"hi"}"#.into(),
            },
        };
        let v = tc.parsed_args().unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "hi");
    }

    #[test]
    fn extract_from_response_message() {
        let msg = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": {"name": "x::y", "arguments": "{\"k\":1}"}
            }]
        });
        let calls = extract_tool_calls(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "c1");
    }

    #[test]
    fn empty_arguments_means_empty_object() {
        let tc = ToolCall {
            id: "c1".into(),
            kind: None,
            function: ToolCallFunction {
                name: "x".into(),
                arguments: "".into(),
            },
        };
        let v = tc.parsed_args().unwrap();
        assert!(v.is_object());
    }
}
