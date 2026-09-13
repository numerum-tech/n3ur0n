//! Persisted instance-level lobe membership (`instance.toml` in the config dir).
//!
//! Precedence for **startup**:
//! 1. Explicit CLI `--lobe` / `N3UR0N_LOBES` when non-empty
//! 2. Saved `instance.toml` lobes when non-empty
//! 3. Empty — the instance joins no lobe, and no capability may claim one
//!
//! The same file backs `GET`/`PUT /api/v0/settings/lobes`. A `PUT` writes the
//! file *and* swaps the live set on the node, so joining a lobe takes effect
//! without a restart.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use n3ur0n_core::validate_instance_lobes;
use serde::{Deserialize, Serialize};

const ENV_LOBES: &str = "N3UR0N_LOBES";

/// The `[instance]` table of `instance.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceUserConfig {
    /// Lobes this instance claims membership of.
    #[serde(default)]
    pub lobe_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstanceUserFile {
    instance: InstanceUserConfig,
}

/// Path of the persisted instance config inside a config dir.
pub fn instance_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("instance.toml")
}

/// Parse a CSV / whitespace-separated lobe list (env or UI field).
pub fn parse_lobe_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Lobes declared through `N3UR0N_LOBES`, if any.
pub fn env_lobes() -> Vec<String> {
    std::env::var(ENV_LOBES)
        .ok()
        .map(|s| parse_lobe_list(&s))
        .unwrap_or_default()
}

/// `Some(cfg)` when the file exists and parses (even with an empty list);
/// `None` when it is missing or malformed.
pub fn load_instance_user_config(config_dir: &Path) -> Option<InstanceUserConfig> {
    let path = instance_config_path(config_dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<InstanceUserFile>(&raw) {
        Ok(file) => Some(file.instance),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "instance.toml parse failed; ignoring file"
            );
            None
        }
    }
}

/// Write `instance.toml`. Validates the set first: a file we would refuse to
/// load is never written.
pub fn save_instance_user_config(config_dir: &Path, cfg: &InstanceUserConfig) -> Result<()> {
    validate_instance_lobes(&cfg.lobe_ids).context("invalid lobe set")?;
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let path = instance_config_path(config_dir);
    let file = InstanceUserFile {
        instance: cfg.clone(),
    };
    let body = toml::to_string_pretty(&file).context("serialising instance.toml")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Resolve the lobe set a starting node should advertise.
///
/// Invalid ids are dropped with a warning rather than aborting startup: a
/// typo in one lobe must not take a gateway offline. The surviving set is
/// truncated to [`MAX_LOBES_PER_INSTANCE`](n3ur0n_core::MAX_LOBES_PER_INSTANCE).
pub fn resolve_startup_lobes(cli_lobes: &[String], config_dir: &Path) -> Vec<String> {
    let mut raw = cli_lobes.to_vec();
    if raw.is_empty() {
        raw = env_lobes();
    }
    if raw.is_empty() {
        raw = load_instance_user_config(config_dir)
            .map(|c| c.lobe_ids)
            .unwrap_or_default();
    }

    let mut kept: Vec<String> = Vec::new();
    for id in raw {
        if let Err(e) = n3ur0n_core::validate_lobe_id(&id) {
            tracing::warn!(error = %e, "ignoring invalid lobe id");
            continue;
        }
        if kept.contains(&id) {
            continue;
        }
        if kept.len() == n3ur0n_core::MAX_LOBES_PER_INSTANCE {
            tracing::warn!(
                lobe = %id,
                max = n3ur0n_core::MAX_LOBES_PER_INSTANCE,
                "too many lobes declared; ignoring the rest"
            );
            break;
        }
        kept.push(id);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cli_wins_over_file() {
        let dir = tempdir().unwrap();
        save_instance_user_config(
            dir.path(),
            &InstanceUserConfig {
                lobe_ids: vec!["from-file".into()],
            },
        )
        .unwrap();
        let lobes = resolve_startup_lobes(&["from-cli".to_string()], dir.path());
        assert_eq!(lobes, vec!["from-cli"]);
    }

    #[test]
    fn falls_back_to_file_then_empty() {
        let dir = tempdir().unwrap();
        assert!(resolve_startup_lobes(&[], dir.path()).is_empty());
        save_instance_user_config(
            dir.path(),
            &InstanceUserConfig {
                lobe_ids: vec!["medical".into(), "legal-fr".into()],
            },
        )
        .unwrap();
        assert_eq!(
            resolve_startup_lobes(&[], dir.path()),
            vec!["medical".to_string(), "legal-fr".to_string()]
        );
    }

    #[test]
    fn drops_invalid_and_duplicate_ids_and_caps_the_count() {
        let dir = tempdir().unwrap();
        let cli: Vec<String> = vec![
            "Medical".into(), // uppercase → rejected
            "ok-one".into(),
            "ok-one".into(), // duplicate → dropped
            "ok-two".into(),
            "ok-three".into(),
            "ok-four".into(),
            "ok-five".into(),
            "ok-six".into(), // past the cap → dropped
        ];
        let lobes = resolve_startup_lobes(&cli, dir.path());
        assert_eq!(lobes.len(), n3ur0n_core::MAX_LOBES_PER_INSTANCE);
        assert!(!lobes.contains(&"Medical".to_string()));
        assert!(!lobes.contains(&"ok-six".to_string()));
    }

    #[test]
    fn refuses_to_write_an_invalid_set() {
        let dir = tempdir().unwrap();
        let err = save_instance_user_config(
            dir.path(),
            &InstanceUserConfig {
                lobe_ids: vec!["NOPE".into()],
            },
        );
        assert!(err.is_err());
        assert!(!instance_config_path(dir.path()).exists());
    }

    #[test]
    fn parses_csv_and_whitespace() {
        assert_eq!(
            parse_lobe_list(" medical, legal-fr\nfinance "),
            vec![
                "medical".to_string(),
                "legal-fr".to_string(),
                "finance".to_string()
            ]
        );
    }
}
