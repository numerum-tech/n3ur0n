//! TOML loader + validator for `backends/*.toml` and `caps/*.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;
use tracing::warn;

use super::types::*;

const SUPPORTED_FORMAT_VERSION: &str = "0.1";

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("toml parse error in {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("manifest validation in {path}: {message}")]
    Validation { path: PathBuf, message: String },
}

impl ManifestError {
    fn validation(path: &Path, message: impl Into<String>) -> Self {
        ManifestError::Validation {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Backend manifests
// ---------------------------------------------------------------------------

/// Parse a single `backends/<name>.toml` file.
pub fn parse_backend_file(path: &Path) -> Result<BackendManifest, ManifestError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file: BackendFile = toml::from_str(&raw).map_err(|source| ManifestError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    validate_format_version(path, &file.manifest.version)?;

    let kind = match file.backend.kind.as_str() {
        "openai_compat" => {
            let cfg = take_openai_compat(path, &file.extras)?;
            BackendKind::OpenAICompat(cfg)
        }
        "mcp_server" => {
            let cfg = take_mcp_server(path, &file.extras)?;
            BackendKind::McpServer(cfg)
        }
        "http_base" => {
            let cfg = take_http_base(path, &file.extras)?;
            BackendKind::HttpBase(cfg)
        }
        other => {
            return Err(ManifestError::validation(
                path,
                format!("unknown backend.kind `{other}` (expected one of: \
openai_compat, mcp_server, http_base)"),
            ));
        }
    };

    let name = file.backend.name.trim();
    if name.is_empty() {
        return Err(ManifestError::validation(path, "backend.name is empty"));
    }

    Ok(BackendManifest {
        name: name.to_string(),
        kind,
    })
}

/// Scan a directory for `*.toml` files. Returns one result per file so the
/// caller can decide whether to abort or continue. A malformed file is
/// logged as a warning and reported as an error in the returned Vec.
pub fn load_backend_dir(dir: &Path) -> Vec<Result<BackendManifest, ManifestError>> {
    list_toml(dir)
        .into_iter()
        .map(|p| parse_backend_file(&p))
        .collect()
}

// ---------------------------------------------------------------------------
// Capability manifests
// ---------------------------------------------------------------------------

/// Parse a single `caps/<name>.toml` file.
pub fn parse_cap_file(path: &Path) -> Result<CapabilityManifest, ManifestError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file: CapFile = toml::from_str(&raw).map_err(|source| ManifestError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    validate_format_version(path, &file.manifest.version)?;
    validate_descriptor(path, &file.descriptor)?;

    let binding = build_binding_spec(path, &file.binding)?;

    Ok(CapabilityManifest {
        descriptor: file.descriptor,
        binding,
    })
}

/// Scan a directory for `*.toml` capability files.
pub fn load_cap_dir(dir: &Path) -> Vec<Result<CapabilityManifest, ManifestError>> {
    list_toml(dir)
        .into_iter()
        .map(|p| parse_cap_file(&p))
        .collect()
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn validate_format_version(path: &Path, version: &str) -> Result<(), ManifestError> {
    if version != SUPPORTED_FORMAT_VERSION {
        return Err(ManifestError::validation(
            path,
            format!(
                "manifest.version is `{version}`; this runtime supports `{SUPPORTED_FORMAT_VERSION}`"
            ),
        ));
    }
    Ok(())
}

fn validate_descriptor(path: &Path, decl: &n3ur0n_core::CapabilityDecl) -> Result<(), ManifestError> {
    if decl.name.trim().is_empty() {
        return Err(ManifestError::validation(path, "descriptor.name is empty"));
    }
    if decl.description.trim().is_empty() {
        return Err(ManifestError::validation(
            path,
            "descriptor.description is empty",
        ));
    }
    // semver
    semver::Version::parse(&decl.version).map_err(|e| {
        ManifestError::validation(
            path,
            format!("descriptor.version `{}` is not valid semver: {e}", decl.version),
        )
    })?;
    // schema_in / schema_out must compile as JSON Schema (or be empty
    // object meaning "accept anything").
    if !decl.schema_in.is_object() {
        return Err(ManifestError::validation(
            path,
            "descriptor.schema_in must be a JSON object",
        ));
    }
    if !decl.schema_out.is_object() {
        return Err(ManifestError::validation(
            path,
            "descriptor.schema_out must be a JSON object",
        ));
    }
    if let Err(e) = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&decl.schema_in)
    {
        return Err(ManifestError::validation(
            path,
            format!("descriptor.schema_in is not a valid JSON Schema: {e}"),
        ));
    }
    if let Err(e) = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&decl.schema_out)
    {
        return Err(ManifestError::validation(
            path,
            format!("descriptor.schema_out is not a valid JSON Schema: {e}"),
        ));
    }
    for lang in &decl.languages {
        if !is_plausible_bcp47(lang) {
            return Err(ManifestError::validation(
                path,
                format!("descriptor.languages contains implausible BCP 47 tag `{lang}`"),
            ));
        }
    }
    for cc in &decl.countries {
        if !is_plausible_iso_country(cc) {
            return Err(ManifestError::validation(
                path,
                format!(
                    "descriptor.countries contains implausible ISO 3166-1 alpha-2 code `{cc}`"
                ),
            ));
        }
    }
    Ok(())
}

fn build_binding_spec(path: &Path, b: &BindingHeader) -> Result<BindingSpec, ManifestError> {
    if b.backend.trim().is_empty() {
        return Err(ManifestError::validation(
            path,
            "binding.backend must reference a backend by name",
        ));
    }
    match b.kind.as_str() {
        "prompt" => {
            let Some(s) = &b.prompt else {
                return Err(ManifestError::validation(
                    path,
                    "binding.type=prompt requires [binding.prompt] section",
                ));
            };
            let parser = match s.output_parser.as_deref().unwrap_or("text") {
                "text" => OutputParser::Text,
                "json" => OutputParser::Json,
                other => {
                    return Err(ManifestError::validation(
                        path,
                        format!("output_parser `{other}` invalid (text|json)"),
                    ));
                }
            };
            Ok(BindingSpec::Prompt {
                backend: b.backend.clone(),
                system_prompt: s.system_prompt.clone(),
                user_template: s.user_template.clone(),
                parameters: s.parameters.clone(),
                output_parser: parser,
                model: s.model.clone(),
            })
        }
        "mcp" => {
            let Some(s) = &b.mcp else {
                return Err(ManifestError::validation(
                    path,
                    "binding.type=mcp requires [binding.mcp] section",
                ));
            };
            if s.tool_name.trim().is_empty() {
                return Err(ManifestError::validation(
                    path,
                    "binding.mcp.tool_name is empty",
                ));
            }
            Ok(BindingSpec::Mcp {
                backend: b.backend.clone(),
                tool_name: s.tool_name.clone(),
                arg_mapping: s.arg_mapping.clone(),
                result_mapping: s.result_mapping.clone(),
            })
        }
        "http" => {
            let Some(s) = &b.http else {
                return Err(ManifestError::validation(
                    path,
                    "binding.type=http requires [binding.http] section",
                ));
            };
            let method = match s.method.to_ascii_uppercase().as_str() {
                "GET" => HttpMethod::Get,
                "POST" => HttpMethod::Post,
                "PUT" => HttpMethod::Put,
                "DELETE" => HttpMethod::Delete,
                other => {
                    return Err(ManifestError::validation(
                        path,
                        format!("http.method `{other}` invalid (GET|POST|PUT|DELETE)"),
                    ));
                }
            };
            Ok(BindingSpec::Http {
                backend: b.backend.clone(),
                url_template: s.url_template.clone(),
                method,
                headers: s.headers.clone(),
                body_template: s.body_template.clone(),
                response_path: s.response_path.clone(),
                timeout_ms: s.timeout_ms,
            })
        }
        other => Err(ManifestError::validation(
            path,
            format!("binding.type `{other}` invalid (prompt|mcp|http)"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Per-kind config extraction
// ---------------------------------------------------------------------------

fn take_openai_compat(
    path: &Path,
    extras: &HashMap<String, Value>,
) -> Result<OpenAICompatConfig, ManifestError> {
    let section = extras
        .get("openai_compat")
        .ok_or_else(|| ManifestError::validation(path, "[openai_compat] section missing"))?
        .as_object()
        .ok_or_else(|| ManifestError::validation(path, "[openai_compat] must be a table"))?;
    let base_url = section
        .get("base_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::validation(path, "openai_compat.base_url required"))?
        .to_string();
    let default_model = section
        .get("default_model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::validation(path, "openai_compat.default_model required"))?
        .to_string();
    let api_key = section
        .get("api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(OpenAICompatConfig {
        base_url,
        default_model,
        api_key,
    })
}

fn take_mcp_server(
    path: &Path,
    extras: &HashMap<String, Value>,
) -> Result<McpServerConfig, ManifestError> {
    let section = extras
        .get("mcp_server")
        .ok_or_else(|| ManifestError::validation(path, "[mcp_server] section missing"))?
        .as_object()
        .ok_or_else(|| ManifestError::validation(path, "[mcp_server] must be a table"))?;
    let transport = match section
        .get("transport")
        .and_then(|v| v.as_str())
        .unwrap_or("stdio")
    {
        "stdio" => McpTransport::Stdio,
        "http_sse" => McpTransport::HttpSse,
        other => {
            return Err(ManifestError::validation(
                path,
                format!("mcp_server.transport `{other}` invalid (stdio|http_sse)"),
            ));
        }
    };
    let command = section
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::validation(path, "mcp_server.command required"))?
        .to_string();
    let args: Vec<String> = section
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env: HashMap<String, String> = section
        .get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(McpServerConfig {
        transport,
        command,
        args,
        env,
    })
}

fn take_http_base(
    path: &Path,
    extras: &HashMap<String, Value>,
) -> Result<HttpBaseConfig, ManifestError> {
    let section = extras
        .get("http_base")
        .ok_or_else(|| ManifestError::validation(path, "[http_base] section missing"))?
        .as_object()
        .ok_or_else(|| ManifestError::validation(path, "[http_base] must be a table"))?;
    let base_url = section
        .get("base_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::validation(path, "http_base.base_url required"))?
        .to_string();
    let headers: HashMap<String, String> = section
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(HttpBaseConfig { base_url, headers })
}

// ---------------------------------------------------------------------------
// Locale validation (light — full BCP 47 / ISO is overkill v0.3.0)
// ---------------------------------------------------------------------------

fn is_plausible_bcp47(tag: &str) -> bool {
    // Accept e.g. "fr", "en-US", "zh-Hans-CN". 2–35 chars, alphanumeric + "-".
    let len = tag.chars().count();
    if !(2..=35).contains(&len) {
        return false;
    }
    tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_plausible_iso_country(code: &str) -> bool {
    code.chars().count() == 2
        && code.chars().all(|c| c.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Directory scan
// ---------------------------------------------------------------------------

fn list_toml(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        warn!(dir = %dir.display(), "manifest directory does not exist or is unreadable");
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) == Some("toml") {
            out.push(path);
        }
    }
    out.sort();
    out
}
