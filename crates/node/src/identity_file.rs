//! On-disk identity file (`keys.json`).
//!
//! v0.1: stored in clear, 0600 on Unix. Encryption deferred to v0.2 per
//! architecture spec §5.3.

use std::fs;
use std::path::{Path, PathBuf};

use data_encoding::HEXLOWER;
use n3ur0n_core::{InstanceId, Keypair};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{NodeError, NodeResult};

/// Persistent shape of `keys.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityFile {
    /// Canonical id derived from the public key. Stored for human readability;
    /// it is recomputed from the secret on load — a stale value warns and
    /// self-heals rather than rejecting the file.
    pub instance_id: InstanceId,
    /// Hex-lower of the 32-byte Ed25519 secret seed.
    pub secret_hex: String,
    /// Hex-lower of the 32-byte Ed25519 public key.
    pub public_hex: String,
}

impl IdentityFile {
    /// Build an identity file from an in-memory keypair.
    pub fn from_keypair(kp: &Keypair) -> Self {
        Self {
            instance_id: kp.instance_id(),
            secret_hex: HEXLOWER.encode(&kp.secret_bytes()),
            public_hex: HEXLOWER.encode(kp.public_key().0.as_bytes()),
        }
    }

    /// Reconstruct the keypair encoded by this file.
    ///
    /// The secret is the source of truth; `instance_id` is a derived,
    /// human-readable cache. A mismatch means the cache is stale — e.g. after
    /// a change to the id-derivation (see the 2026-07-18 truncation) — or the
    /// field was hand-edited. Neither affects identity, since the running id is
    /// always recomputed from the secret. We therefore warn and self-heal
    /// rather than reject, so existing `keys.json` files survive the change.
    pub fn into_keypair(self) -> NodeResult<Keypair> {
        let bytes = HEXLOWER
            .decode(self.secret_hex.as_bytes())
            .map_err(|e| NodeError::Identity(format!("hex secret: {e}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| NodeError::Identity("secret must be 32 bytes".into()))?;
        let kp = Keypair::from_secret_bytes(&arr);
        if kp.instance_id() != self.instance_id {
            warn!(
                stored = %self.instance_id,
                derived = %kp.instance_id(),
                "keys.json instance_id is stale; using secret-derived id"
            );
        }
        Ok(kp)
    }

    /// Load an identity file from disk and reconstruct the keypair.
    ///
    /// # Errors
    /// IO failure, JSON parse error, or hash mismatch between stored
    /// `instance_id` and the secret-derived id.
    pub fn load(path: &Path) -> NodeResult<Keypair> {
        let raw = fs::read_to_string(path)?;
        let file: Self = serde_json::from_str(&raw)?;
        file.into_keypair()
    }

    /// Persist this identity file with restrictive permissions on Unix.
    ///
    /// Refuses to overwrite an existing file. Caller decides whether to
    /// delete first; this avoids accidental key clobber.
    pub fn save(&self, path: &Path) -> NodeResult<()> {
        if path.exists() {
            return Err(NodeError::Identity(format!(
                "refusing to overwrite existing identity at {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = fs::metadata(path)?.permissions();
            perm.set_mode(0o600);
            fs::set_permissions(path, perm)?;
        }
        Ok(())
    }
}

/// Convenience: identity file path inside a config directory.
pub fn default_path(config_dir: &Path) -> PathBuf {
    config_dir.join("keys.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn save_then_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = default_path(dir.path());
        let kp = Keypair::generate();
        let file = IdentityFile::from_keypair(&kp);
        file.save(&path).unwrap();
        let loaded = IdentityFile::load(&path).unwrap();
        assert_eq!(loaded.instance_id(), kp.instance_id());
    }

    #[test]
    fn refuses_to_overwrite() {
        let dir = tempdir().unwrap();
        let path = default_path(dir.path());
        IdentityFile::from_keypair(&Keypair::generate())
            .save(&path)
            .unwrap();
        let err = IdentityFile::from_keypair(&Keypair::generate())
            .save(&path)
            .unwrap_err();
        assert!(matches!(err, NodeError::Identity(_)));
    }

    #[test]
    fn self_heals_stale_instance_id() {
        // A stale/edited instance_id must not brick the file: the secret is
        // authoritative and the loaded keypair derives the correct id.
        let kp = Keypair::generate();
        let file = IdentityFile {
            instance_id: Keypair::generate().instance_id(), // wrong/stale
            secret_hex: HEXLOWER.encode(&kp.secret_bytes()),
            public_hex: HEXLOWER.encode(kp.public_key().0.as_bytes()),
        };
        let loaded = file.into_keypair().expect("loads despite stale id");
        assert_eq!(loaded.instance_id(), kp.instance_id());
    }
}
