//! Tiny templating for `{{args.dotted.path}}` substitution.
//!
//! Used by all three binding kinds (prompt user_template, http url_template
//! and body_template, mcp arg_mapping). Intentionally **not** Jinja:
//!
//! - one syntax (`{{args.path.to.value}}`), no conditionals, no loops.
//! - whole-string `{{args.x}}` → returns the resolved JSON value (preserves
//!   numbers / objects); useful for body_template fields.
//! - inline `... {{args.x}} ...` → string interpolation; non-string values
//!   are JSON-serialised before insertion.
//! - missing path → returns a structured error so the binding can fail the
//!   call cleanly rather than send literal `{{...}}` to a downstream tool.

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template references `{path}` which is not present in args")]
    PathNotFound { path: String },
    #[error("template references `{name}` which is not a known root (expected `args`)")]
    UnknownRoot { name: String },
}

/// Resolve every `{{args.path}}` occurrence in `template` against `args`.
/// Whole-string single template → returns the resolved value as-is; inline
/// substitution → returns a `Value::String` with text interpolation.
pub fn render(template: &str, args: &Value) -> Result<Value, TemplateError> {
    let trimmed = template.trim();
    if let Some(inner) = whole_template(trimmed) {
        let (root, path) = split_root(inner)?;
        check_root(root)?;
        return resolve_path(args, path).cloned().ok_or_else(|| {
            TemplateError::PathNotFound {
                path: format!("{root}.{path}"),
            }
        });
    }
    Ok(Value::String(substitute_inline(template, args)?))
}

/// Recursively render a `Value` tree: strings are passed through `render`,
/// arrays and objects are walked.
pub fn render_value(template: &Value, args: &Value) -> Result<Value, TemplateError> {
    match template {
        Value::String(s) => render(s, args),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(render_value(v, args)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(o) => {
            let mut out = serde_json::Map::with_capacity(o.len());
            for (k, v) in o {
                out.insert(k.clone(), render_value(v, args)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn whole_template(s: &str) -> Option<&str> {
    // Match `{{...}}` covering the entire trimmed string.
    if s.starts_with("{{") && s.ends_with("}}") {
        let inner = &s[2..s.len() - 2].trim();
        // Reject if there is another `{{` inside (mixed inline/whole form).
        if !inner.contains("{{") {
            return Some(inner);
        }
    }
    None
}

fn split_root(path: &str) -> Result<(&str, &str), TemplateError> {
    match path.split_once('.') {
        Some((root, rest)) => Ok((root, rest)),
        None => Err(TemplateError::PathNotFound {
            path: path.to_string(),
        }),
    }
}

fn check_root(root: &str) -> Result<(), TemplateError> {
    if root != "args" {
        return Err(TemplateError::UnknownRoot {
            name: root.to_string(),
        });
    }
    Ok(())
}

fn resolve_path<'a>(args: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = args;
    for segment in dotted.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    Some(current)
}

fn substitute_inline(s: &str, args: &Value) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find closing `}}`.
            let Some(rel_end) = s[i + 2..].find("}}") else {
                // Unbalanced — preserve raw text from here.
                out.push_str(&s[i..]);
                break;
            };
            let inner = s[i + 2..i + 2 + rel_end].trim();
            let (root, path) = split_root(inner)?;
            check_root(root)?;
            let resolved = resolve_path(args, path).ok_or_else(|| {
                TemplateError::PathNotFound {
                    path: format!("{root}.{path}"),
                }
            })?;
            out.push_str(&value_to_text(resolved));
            i = i + 2 + rel_end + 2;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_template_returns_value() {
        let args = json!({"text": "bonjour", "n": 42});
        assert_eq!(render("{{args.text}}", &args).unwrap(), json!("bonjour"));
        assert_eq!(render("{{args.n}}", &args).unwrap(), json!(42));
    }

    #[test]
    fn inline_interpolation_renders_string() {
        let args = json!({"city": "Lomé"});
        let out = render("Weather in {{args.city}} please.", &args).unwrap();
        assert_eq!(out, json!("Weather in Lomé please."));
    }

    #[test]
    fn dotted_path() {
        let args = json!({"user": {"profile": {"lang": "fr"}}});
        let out = render("{{args.user.profile.lang}}", &args).unwrap();
        assert_eq!(out, json!("fr"));
    }

    #[test]
    fn missing_path_errors() {
        let args = json!({});
        let err = render("{{args.nope}}", &args).unwrap_err();
        assert!(matches!(err, TemplateError::PathNotFound { .. }));
    }

    #[test]
    fn unknown_root_errors() {
        let args = json!({"x": 1});
        let err = render("{{secret.x}}", &args).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownRoot { .. }));
    }

    #[test]
    fn render_value_walks_nested_structures() {
        let args = json!({"who": "world"});
        let tmpl = json!({
            "greeting": "hello {{args.who}}",
            "list": ["{{args.who}}", "static"]
        });
        let out = render_value(&tmpl, &args).unwrap();
        assert_eq!(out["greeting"], "hello world");
        assert_eq!(out["list"][0], "world");
        assert_eq!(out["list"][1], "static");
    }

    #[test]
    fn no_template_returns_value_string() {
        let args = json!({});
        let out = render("literal", &args).unwrap();
        assert_eq!(out, json!("literal"));
    }
}
