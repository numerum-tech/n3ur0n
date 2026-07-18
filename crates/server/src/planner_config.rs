//! User-facing planner model selection (`planner.toml` in the config dir).
//!
//! Env vars (`N3UR0N_PLANNER_LLM_*`) provide the minimum bootstrap default.
//! Operators can point the planner at a named `openai_compat` manifest
//! backend for better performance without duplicating URLs in env.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use n3ur0n_adapters::openai::{OpenAIConfig, normalize_openai_base_url};
use n3ur0n_node::Node;
use n3ur0n_node::manifest::{BackendKind, OpenAICompatConfig, load_backend_dir};
use n3ur0n_node::runtime::{NodeRuntime, RuntimeConfig};
use serde::{Deserialize, Serialize};

/// Fallback LLM endpoint from CLI / env at startup.
#[derive(Debug, Clone)]
pub struct PlannerEnvFallback {
    pub base_url: String,
    pub default_model: String,
    pub api_key: Option<String>,
}

/// Persisted operator preference (`<config>/planner.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannerUserConfig {
    /// Name of a backend declared in `backends/<name>.toml`. Empty / absent
    /// means "use env fallback".
    #[serde(default)]
    pub backend: Option<String>,
    /// Optional model override (within the selected backend or env default).
    #[serde(default)]
    pub model: Option<String>,
}

pub fn planner_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("planner.toml")
}

pub fn load_planner_user_config(config_dir: &Path) -> PlannerUserConfig {
    let path = planner_config_path(config_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return PlannerUserConfig::default();
    };
    match toml::from_str::<PlannerUserFile>(&raw) {
        Ok(file) => file.planner,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "planner.toml parse failed; using env default");
            PlannerUserConfig::default()
        }
    }
}

pub fn save_planner_user_config(config_dir: &Path, cfg: &PlannerUserConfig) -> Result<()> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let path = planner_config_path(config_dir);
    let file = PlannerUserFile {
        planner: cfg.clone(),
    };
    let body = toml::to_string_pretty(&file).context("serialising planner.toml")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct PlannerUserFile {
    planner: PlannerUserConfig,
}

/// Handle for hot-swapping the planner runtime when backend manifests change.
#[derive(Clone, Debug)]
pub struct PlannerRuntimeHandle {
    pub runtime: Arc<ArcSwap<Option<Arc<NodeRuntime>>>>,
    pub env: PlannerEnvFallback,
    pub runtime_config: RuntimeConfig,
}

/// Rebuild the in-memory planner runtime when a saved backend is the one
/// selected in `planner.toml`. No-op when the planner is disabled or the
/// backend is unrelated.
pub fn hot_reload_planner_after_backend_change(
    node: &Node,
    config_dir: &Path,
    handle: &PlannerRuntimeHandle,
    backend_name: &str,
) -> Result<Option<ResolvedPlannerLlm>> {
    if handle.runtime.load_full().is_none() {
        return Ok(None);
    }
    let user = load_planner_user_config(config_dir);
    if user.backend.as_deref() != Some(backend_name) {
        return Ok(None);
    }
    hot_reload_planner_runtime(node, config_dir, handle, &user).map(Some)
}

/// Rebuild planner runtime from the given user config.
pub fn hot_reload_planner_runtime(
    node: &Node,
    config_dir: &Path,
    handle: &PlannerRuntimeHandle,
    user: &PlannerUserConfig,
) -> Result<ResolvedPlannerLlm> {
    let rt = crate::bootstrap::build_runtime_with_user_config(
        node.clone(),
        config_dir,
        &handle.env,
        user,
        handle.runtime_config.clone(),
    )?;
    let resolved = resolve_planner_llm(node, config_dir, &handle.env, user)?;
    handle.runtime.store(Arc::new(Some(Arc::new(rt))));
    tracing::info!(
        source = resolved.source,
        backend = ?resolved.backend_name,
        base_url = %resolved.openai.base_url,
        model = %resolved.model_hint,
        "planner runtime hot-reloaded after backend change"
    );
    Ok(resolved)
}

/// Resolved OpenAI-compatible config for the planner runtime.
#[derive(Debug, Clone)]
pub struct ResolvedPlannerLlm {
    pub openai: OpenAIConfig,
    pub model_hint: String,
    /// `"backend"` or `"env"`.
    pub source: &'static str,
    /// Set when `source == "backend"`.
    pub backend_name: Option<String>,
}

/// Resolve which LLM the planner should use.
pub fn resolve_planner_llm(
    node: &Node,
    config_dir: &Path,
    env: &PlannerEnvFallback,
    user: &PlannerUserConfig,
) -> Result<ResolvedPlannerLlm> {
    let backend_name = user
        .backend
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(name) = backend_name.clone() {
        let cfg = load_openai_compat_backend(config_dir, &name)
            .with_context(|| format!("planner backend `{name}`"))?;
        if node.is_manifest_mode() && !node.has_openai_compat_backend(&name) {
            anyhow::bail!(
                "backend `{name}` is not loaded (add it under Settings → Backends, or reload after edits)"
            );
        }
        let model_hint = user
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| cfg.default_model.clone());
        let openai = OpenAIConfig {
            base_url: cfg.base_url,
            default_model: model_hint.clone(),
            api_key: if cfg.api_key.is_empty() {
                None
            } else {
                Some(cfg.api_key)
            },
            description: None,
            allow_model_override: false,
        };
        return Ok(ResolvedPlannerLlm {
            openai,
            model_hint,
            source: "backend",
            backend_name: Some(name),
        });
    }

    let model_hint = user
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| env.default_model.clone());
    let openai = OpenAIConfig {
        base_url: normalize_openai_base_url(&env.base_url),
        default_model: model_hint.clone(),
        api_key: env.api_key.clone(),
        description: None,
        allow_model_override: false,
    };
    Ok(ResolvedPlannerLlm {
        openai,
        model_hint,
        source: "env",
        backend_name: None,
    })
}

fn load_openai_compat_backend(config_dir: &Path, name: &str) -> Result<OpenAICompatConfig> {
    let path = config_dir.join("backends").join(format!("{name}.toml"));
    let manifest = n3ur0n_node::manifest::parse_backend_file(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    if manifest.name != name {
        tracing::warn!(
            file = %path.display(),
            expected = %name,
            found = %manifest.name,
            "backend manifest name mismatch"
        );
    }
    match manifest.kind {
        BackendKind::OpenAICompat(cfg) => Ok(cfg),
        other => anyhow::bail!("backend `{name}` is {other:?}, not openai_compat"),
    }
}

/// List `openai_compat` backend names from disk (for the settings picker).
pub fn list_openai_compat_backends(config_dir: &Path) -> Vec<serde_json::Value> {
    let dir = config_dir.join("backends");
    let mut out = Vec::new();
    for result in load_backend_dir(&dir) {
        let Ok(m) = result else { continue };
        if let BackendKind::OpenAICompat(cfg) = m.kind {
            out.push(serde_json::json!({
                "name": m.name,
                "kind": "openai_compat",
                "base_url": cfg.base_url,
                "default_model": cfg.default_model,
            }));
        }
    }
    out.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use n3ur0n_adapters::{Backend, echo::EchoBackend};
    use n3ur0n_node::{CapabilityRegistry, NodeConfig};
    use tempfile::TempDir;

    fn test_node() -> Node {
        let kp = n3ur0n_core::Keypair::generate();
        let db = n3ur0n_storage::open_in_memory().unwrap();
        let backend: std::sync::Arc<dyn Backend> = std::sync::Arc::new(EchoBackend);
        let registry = CapabilityRegistry::from_decls(vec![]);
        n3ur0n_node::Node::new(kp, db, backend, registry, NodeConfig::default())
    }

    #[test]
    fn resolve_env_default_without_user_file() {
        let dir = TempDir::new().unwrap();
        let env = PlannerEnvFallback {
            base_url: "http://localhost:11434".into(),
            default_model: "llama3.1:8b".into(),
            api_key: None,
        };
        let node = test_node();
        let r =
            resolve_planner_llm(&node, dir.path(), &env, &PlannerUserConfig::default()).unwrap();
        assert_eq!(r.source, "env");
        assert_eq!(r.model_hint, "llama3.1:8b");
        assert_eq!(r.openai.base_url, "http://localhost:11434");
    }

    #[test]
    fn resolve_named_backend() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("backends")).unwrap();
        std::fs::write(
            dir.path().join("backends/ollama-big.toml"),
            r#"
[manifest]
version = "0.1"

[backend]
name = "ollama-big"
kind = "openai_compat"

[openai_compat]
base_url = "http://localhost:11434"
default_model = "qwen2.5:14b"
api_key = ""
"#,
        )
        .unwrap();

        let env = PlannerEnvFallback {
            base_url: "http://localhost:11434".into(),
            default_model: "llama3.1:8b".into(),
            api_key: None,
        };
        let node = test_node();
        let user = PlannerUserConfig {
            backend: Some("ollama-big".into()),
            model: None,
        };
        // Non-manifest nodes resolve from disk without a live registry.
        let r = resolve_planner_llm(&node, dir.path(), &env, &user).unwrap();
        assert_eq!(r.source, "backend");
        assert_eq!(r.model_hint, "qwen2.5:14b");

        // Manifest mode requires the backend to be loaded in-memory.
        use n3ur0n_node::backends_registry::BackendsRegistry;
        use n3ur0n_node::manifest::parse_backend_file;
        let m = parse_backend_file(&dir.path().join("backends/ollama-big.toml")).unwrap();
        let reg = BackendsRegistry::from_manifests([m]).unwrap();
        let node = node.with_manifest_runtime(std::sync::Arc::new(reg), dir.path().to_path_buf());

        let r = resolve_planner_llm(&node, dir.path(), &env, &user).unwrap();
        assert_eq!(r.source, "backend");
        assert_eq!(r.backend_name.as_deref(), Some("ollama-big"));
        assert_eq!(r.model_hint, "qwen2.5:14b");
    }
}
