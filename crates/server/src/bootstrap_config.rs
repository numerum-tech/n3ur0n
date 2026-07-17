//! Persisted bootstrap seed peers (`bootstrap.toml` in the config dir).
//!
//! Precedence for **startup**:
//! 1. Explicit CLI / `N3UR0N_BOOTSTRAP_PEERS` when non-empty
//! 2. Saved `bootstrap.toml` peers when non-empty
//! 3. Empty
//!
//! Saved peers are also exposed via `GET /api/v0/settings/bootstrap` for the
//! Gateways “add” flow (merge on save). The add form itself is not prefilled.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Public default seed advertised in the UI helper button.
pub const PUBLIC_SEED_ENDPOINT: &str = "https://seed.n3ur0n.net";

const ENV_BOOTSTRAP_PEERS: &str = "N3UR0N_BOOTSTRAP_PEERS";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapUserConfig {
    #[serde(default)]
    pub peers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BootstrapUserFile {
    bootstrap: BootstrapUserConfig,
}

pub fn bootstrap_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("bootstrap.toml")
}

/// Parse CSV / whitespace-separated endpoint list (env or UI textarea).
pub fn parse_peer_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn env_bootstrap_peers() -> Vec<String> {
    std::env::var(ENV_BOOTSTRAP_PEERS)
        .ok()
        .map(|s| parse_peer_list(&s))
        .unwrap_or_default()
}

/// `Some(cfg)` when the file exists (even if peers empty); `None` if missing.
pub fn load_bootstrap_user_config(config_dir: &Path) -> Option<BootstrapUserConfig> {
    let path = bootstrap_config_path(config_dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<BootstrapUserFile>(&raw) {
        Ok(file) => Some(file.bootstrap),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "bootstrap.toml parse failed; ignoring file"
            );
            None
        }
    }
}

pub fn save_bootstrap_user_config(config_dir: &Path, cfg: &BootstrapUserConfig) -> Result<()> {
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let path = bootstrap_config_path(config_dir);
    let file = BootstrapUserFile {
        bootstrap: cfg.clone(),
    };
    let body = toml::to_string_pretty(&file).context("serialising bootstrap.toml")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Peers to show in the Settings form.
pub fn form_bootstrap_peers(config_dir: &Path) -> (Vec<String>, &'static str) {
    if let Some(cfg) = load_bootstrap_user_config(config_dir) {
        return (normalize_peers(cfg.peers), "file");
    }
    let env = env_bootstrap_peers();
    if !env.is_empty() {
        return (normalize_peers(env), "env");
    }
    (Vec::new(), "empty")
}

/// Peers used at process startup when CLI list is empty.
pub fn startup_bootstrap_peers(config_dir: &Path) -> Vec<String> {
    if let Some(cfg) = load_bootstrap_user_config(config_dir) {
        let peers = normalize_peers(cfg.peers);
        if !peers.is_empty() {
            return peers;
        }
    }
    Vec::new()
}

/// Resolve startup list: explicit CLI/env wins, else saved file.
pub fn resolve_startup_peers(cli_peers: &[String], config_dir: &Path) -> Vec<String> {
    let cli = normalize_peers(cli_peers.to_vec());
    if !cli.is_empty() {
        return cli;
    }
    startup_bootstrap_peers(config_dir)
}

fn normalize_peers(peers: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for p in peers {
        let t = p.trim().trim_end_matches('/').to_string();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == &t) {
            out.push(t);
        }
    }
    out
}

/// Deduplicate / trim a peer list without reading disk.
pub fn normalize_peer_list(peers: Vec<String>) -> Vec<String> {
    normalize_peers(peers)
}

/// Remove `endpoint` from saved `bootstrap.toml` (no-op if file missing).
/// Returns whether the file was rewritten.
pub fn remove_endpoint_from_bootstrap(config_dir: &Path, endpoint: &str) -> Result<bool> {
    let Some(cfg) = load_bootstrap_user_config(config_dir) else {
        return Ok(false);
    };
    let target = endpoint.trim().trim_end_matches('/');
    let before = cfg.peers.len();
    let peers: Vec<String> = cfg
        .peers
        .into_iter()
        .filter(|p| p.trim().trim_end_matches('/') != target)
        .collect();
    if peers.len() == before {
        return Ok(false);
    }
    save_bootstrap_user_config(config_dir, &BootstrapUserConfig { peers })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_peer_list_csv_and_newlines() {
        let v = parse_peer_list(" https://a.example ,\nhttps://b.example\n ");
        assert_eq!(v, vec!["https://a.example", "https://b.example"]);
    }

    #[test]
    fn form_uses_file_when_present() {
        let dir = tempdir().unwrap();
        save_bootstrap_user_config(
            dir.path(),
            &BootstrapUserConfig {
                peers: vec!["https://from-file.example".into()],
            },
        )
        .unwrap();
        let (peers, source) = form_bootstrap_peers(dir.path());
        assert_eq!(source, "file");
        assert_eq!(peers, vec!["https://from-file.example"]);
    }

    #[test]
    fn form_empty_without_file() {
        let dir = tempdir().unwrap();
        let (peers, source) = form_bootstrap_peers(dir.path());
        // May be "env" if the process happens to have N3UR0N_BOOTSTRAP_PEERS set.
        if source == "empty" {
            assert!(peers.is_empty());
        }
    }

    #[test]
    fn resolve_cli_beats_file() {
        let dir = tempdir().unwrap();
        save_bootstrap_user_config(
            dir.path(),
            &BootstrapUserConfig {
                peers: vec!["https://from-file.example".into()],
            },
        )
        .unwrap();
        let peers = resolve_startup_peers(&["https://from-cli.example".into()], dir.path());
        assert_eq!(peers, vec!["https://from-cli.example"]);
    }

    #[test]
    fn resolve_falls_back_to_file() {
        let dir = tempdir().unwrap();
        save_bootstrap_user_config(
            dir.path(),
            &BootstrapUserConfig {
                peers: vec!["https://from-file.example".into()],
            },
        )
        .unwrap();
        let peers = resolve_startup_peers(&[], dir.path());
        assert_eq!(peers, vec!["https://from-file.example"]);
    }

    #[test]
    fn remove_endpoint_rewrites_file() {
        let dir = tempdir().unwrap();
        save_bootstrap_user_config(
            dir.path(),
            &BootstrapUserConfig {
                peers: vec![
                    "https://a.example/".into(),
                    "https://b.example".into(),
                ],
            },
        )
        .unwrap();
        assert!(remove_endpoint_from_bootstrap(dir.path(), "https://a.example").unwrap());
        let cfg = load_bootstrap_user_config(dir.path()).unwrap();
        assert_eq!(cfg.peers, vec!["https://b.example"]);
        assert!(!remove_endpoint_from_bootstrap(dir.path(), "https://missing.example").unwrap());
    }
}
