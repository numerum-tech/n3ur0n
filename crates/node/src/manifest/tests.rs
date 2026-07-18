//! Parser tests for the v0.3 manifest format.
//!
//! Fixtures mirror the three canonical examples from
//! `n3ur0n-capability-manifest-v0.md` §3 plus the backend-config split we
//! introduced on top of the spec (backends/*.toml separate from caps/*.toml).

use super::parser::*;
use super::types::*;
use std::path::Path;
use tempfile::TempDir;

fn write(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, body).unwrap();
    p
}

// ---------------------------------------------------------------------------
// Backend manifests
// ---------------------------------------------------------------------------

#[test]
fn parse_backend_openai_compat() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "local_ollama.toml",
        r#"
[manifest]
version = "0.1"

[backend]
name = "local_ollama"
kind = "openai_compat"

[openai_compat]
base_url      = "http://localhost:11434"
default_model = "llama3.1:8b"
api_key       = ""
"#,
    );
    let m = parse_backend_file(&path).unwrap();
    assert_eq!(m.name, "local_ollama");
    match m.kind {
        BackendKind::OpenAICompat(cfg) => {
            assert_eq!(cfg.base_url, "http://localhost:11434");
            assert_eq!(cfg.default_model, "llama3.1:8b");
        }
        _ => panic!("wrong kind"),
    }
}

#[test]
fn parse_backend_mcp_stdio() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "github_mcp.toml",
        r#"
[manifest]
version = "0.1"

[backend]
name = "github_mcp"
kind = "mcp_server"

[mcp_server]
transport = "stdio"
command   = "github-mcp-server"
args      = ["--readonly"]
env       = { GITHUB_TOKEN = "ghp_xxx" }
"#,
    );
    let m = parse_backend_file(&path).unwrap();
    match m.kind {
        BackendKind::McpServer(cfg) => {
            assert_eq!(cfg.transport, McpTransport::Stdio);
            assert_eq!(cfg.command, "github-mcp-server");
            assert_eq!(cfg.args, vec!["--readonly"]);
            assert_eq!(cfg.env.get("GITHUB_TOKEN").unwrap(), "ghp_xxx");
        }
        _ => panic!("wrong kind"),
    }
}

#[test]
fn parse_backend_http_base() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "openmeteo.toml",
        r#"
[manifest]
version = "0.1"

[backend]
name = "openmeteo"
kind = "http_base"

[http_base]
base_url = "https://api.open-meteo.com/v1"
headers  = { "User-Agent" = "n3ur0n/0.3" }
"#,
    );
    let m = parse_backend_file(&path).unwrap();
    match m.kind {
        BackendKind::HttpBase(cfg) => {
            assert_eq!(cfg.base_url, "https://api.open-meteo.com/v1");
            assert_eq!(cfg.headers.get("User-Agent").unwrap(), "n3ur0n/0.3");
        }
        _ => panic!("wrong kind"),
    }
}

#[test]
fn backend_unknown_kind_rejected() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "bad.toml",
        r#"
[manifest]
version = "0.1"

[backend]
name = "bad"
kind = "what"
"#,
    );
    let err = parse_backend_file(&path).unwrap_err().to_string();
    assert!(err.contains("unknown backend.kind"), "{err}");
}

#[test]
fn backend_wrong_format_version_rejected() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "future.toml",
        r#"
[manifest]
version = "9.9"

[backend]
name = "future"
kind = "openai_compat"

[openai_compat]
base_url = "x"
default_model = "y"
"#,
    );
    let err = parse_backend_file(&path).unwrap_err().to_string();
    assert!(err.contains("manifest.version"), "{err}");
}

// ---------------------------------------------------------------------------
// Capability manifests
// ---------------------------------------------------------------------------

fn translator_cap() -> &'static str {
    r#"
[manifest]
version = "0.1"

[descriptor]
name = "translator-fr-en"
version = "1.0.0"
description = "Traduit du texte court du français vers l'anglais avec un style neutre."
mode = "free"
tags = ["translation", "language", "fr", "en"]
lobe_ids = ["lobe.community.translators.v1"]
languages = ["fr", "en"]
countries = ["FR", "BE", "CH"]
disambiguation = "Préférer cette cap à `chat` pour une traduction littérale."

[descriptor.schema_in]
type = "object"
required = ["text"]
properties = { text = { type = "string", maxLength = 4000 } }

[descriptor.schema_out]
type = "object"
required = ["translation"]
properties = { translation = { type = "string" } }

[[descriptor.examples]]
user_intent = "traduire 'bonjour'"
args = { text = "bonjour" }
expected_output = { translation = "hello" }

[binding]
type = "prompt"
backend = "local_ollama"

[binding.prompt]
system_prompt = "Tu es un traducteur littéral fr→en."
user_template = "Traduire : {{args.text}}"
parameters    = { temperature = 0.0 }
output_parser = "json"
"#
}

#[test]
fn parse_cap_prompt_full() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, "translator.toml", translator_cap());
    let m = parse_cap_file(&path).unwrap();
    assert_eq!(m.descriptor.name, "translator-fr-en");
    assert_eq!(m.descriptor.version, "1.0.0");
    assert_eq!(m.descriptor.languages, vec!["fr", "en"]);
    assert_eq!(m.descriptor.countries, vec!["FR", "BE", "CH"]);
    match m.binding {
        BindingSpec::Prompt {
            backend,
            output_parser,
            user_template,
            ..
        } => {
            assert_eq!(backend, "local_ollama");
            assert_eq!(output_parser, OutputParser::Json);
            assert!(user_template.unwrap().contains("{{args.text}}"));
        }
        _ => panic!("wrong binding"),
    }
}

#[test]
fn parse_cap_mcp_binding() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "github-search.toml",
        r#"
[manifest]
version = "0.1"

[descriptor]
name = "github-search-issues"
version = "0.1.0"
description = "Search GitHub issues."
mode = "free"

[descriptor.schema_in]
type = "object"
required = ["query"]
properties = { query = { type = "string" } }

[descriptor.schema_out]
type = "object"
required = ["issues"]
properties = { issues = { type = "array", items = { type = "object" } } }

[[descriptor.examples]]
user_intent = "find open issues"
args = { query = "leak" }
expected_output = { issues = [] }

[binding]
type = "mcp"
backend = "github_mcp"

[binding.mcp]
tool_name = "search_issues"
"#,
    );
    let m = parse_cap_file(&path).unwrap();
    match m.binding {
        BindingSpec::Mcp {
            backend, tool_name, ..
        } => {
            assert_eq!(backend, "github_mcp");
            assert_eq!(tool_name, "search_issues");
        }
        _ => panic!("wrong binding"),
    }
}

#[test]
fn parse_cap_http_binding() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "weather.toml",
        r#"
[manifest]
version = "0.1"

[descriptor]
name = "weather-now"
version = "0.1.0"
description = "Current weather for a city."
mode = "free"

[descriptor.schema_in]
type = "object"
required = ["city"]
properties = { city = { type = "string" } }

[descriptor.schema_out]
type = "object"
required = ["temperature_c"]
properties = { temperature_c = { type = "number" } }

[[descriptor.examples]]
user_intent = "weather in Lomé"
args = { city = "Lomé" }
expected_output = { temperature_c = 30.5 }

[binding]
type = "http"
backend = "openmeteo"

[binding.http]
url_template = "/forecast?q={{args.city}}"
method       = "GET"
response_path = "$.current_weather"
"#,
    );
    let m = parse_cap_file(&path).unwrap();
    match m.binding {
        BindingSpec::Http {
            backend,
            method,
            response_path,
            url_template,
            ..
        } => {
            assert_eq!(backend, "openmeteo");
            assert_eq!(method, HttpMethod::Get);
            assert_eq!(response_path.as_deref(), Some("$.current_weather"));
            assert!(url_template.contains("{{args.city}}"));
        }
        _ => panic!("wrong binding"),
    }
}

#[test]
fn cap_rejects_non_semver_version() {
    let dir = TempDir::new().unwrap();
    let bad = translator_cap().replace(r#"version = "1.0.0""#, r#"version = "vNEXT""#);
    let path = write(&dir, "bad.toml", &bad);
    let err = parse_cap_file(&path).unwrap_err().to_string();
    assert!(err.contains("semver"), "{err}");
}

#[test]
fn cap_rejects_implausible_country_code() {
    let dir = TempDir::new().unwrap();
    let bad = translator_cap().replace(
        r#"countries = ["FR", "BE", "CH"]"#,
        r#"countries = ["France"]"#,
    );
    let path = write(&dir, "bad.toml", &bad);
    let err = parse_cap_file(&path).unwrap_err().to_string();
    assert!(err.contains("ISO 3166"), "{err}");
}

#[test]
fn cap_rejects_invalid_schema_in() {
    let dir = TempDir::new().unwrap();
    let bad = translator_cap().replace(
        r#"type = "object"
required = ["text"]
properties = { text = { type = "string", maxLength = 4000 } }"#,
        r#"type = "not-a-real-json-schema-type-zzz"
properties = { text = { type = "object", required = "not-an-array" } }"#,
    );
    let path = write(&dir, "bad.toml", &bad);
    let err = parse_cap_file(&path).unwrap_err().to_string();
    assert!(err.contains("schema_in") || err.contains("Schema"), "{err}");
}

#[test]
fn cap_rejects_binding_type_without_section() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        "missing.toml",
        r#"
[manifest]
version = "0.1"

[descriptor]
name = "x"
version = "0.1.0"
description = "x"
mode = "free"

[descriptor.schema_in]
type = "object"

[descriptor.schema_out]
type = "object"

[[descriptor.examples]]
user_intent = "x"
args = {}
expected_output = {}

[binding]
type = "mcp"
backend = "any"
"#,
    );
    let err = parse_cap_file(&path).unwrap_err().to_string();
    assert!(err.contains("[binding.mcp]"), "{err}");
}

// ---------------------------------------------------------------------------
// Directory scan
// ---------------------------------------------------------------------------

#[test]
fn load_cap_dir_skips_non_toml() {
    let dir = TempDir::new().unwrap();
    let _ = write(&dir, "translator.toml", translator_cap());
    let _ = write(&dir, "README.md", "ignored");
    let _ = std::fs::create_dir_all(dir.path().join("subdir"));
    let results = load_cap_dir(dir.path());
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
}

#[test]
fn load_cap_dir_missing_dir_returns_empty() {
    let results = load_cap_dir(Path::new("/this/path/definitely/does/not/exist"));
    assert!(results.is_empty());
}
