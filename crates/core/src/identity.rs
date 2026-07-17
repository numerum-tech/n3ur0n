//! Cryptographic identity for an n3ur0n instance.
//!
//! The canonical instance id is `n3:` + Base32(SHA-256(Ed25519 public key))
//! using RFC 4648 alphabet without padding. Any holder of the public key can
//! recompute it; no registry is required.

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

/// Canonical identifier prefix.
pub const ID_PREFIX: &str = "n3:";

/// A canonical instance identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(String);

impl InstanceId {
    /// Derive an instance id from an Ed25519 public key.
    pub fn from_public_key(pk: &VerifyingKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(pk.as_bytes());
        let digest = hasher.finalize();
        let encoded = BASE32_NOPAD.encode(&digest).to_lowercase();
        Self(format!("{ID_PREFIX}{encoded}"))
    }

    /// String view of the identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse a string into an `InstanceId`. Validates prefix and charset only;
    /// does not verify that a public key with this hash exists on the network.
    pub fn parse(s: &str) -> CoreResult<Self> {
        if !s.starts_with(ID_PREFIX) {
            return Err(CoreError::InvalidIdentifier(s.to_string()));
        }
        let payload = &s[ID_PREFIX.len()..];
        if payload.is_empty() || !payload.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(CoreError::InvalidIdentifier(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Wrapper around an Ed25519 verifying key with serde support.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey(#[serde(with = "verifying_key_bytes")] pub VerifyingKey);

impl PublicKey {
    /// Derive the canonical instance id of this key.
    pub fn instance_id(&self) -> InstanceId {
        InstanceId::from_public_key(&self.0)
    }

    /// Verify a signature over `message`.
    ///
    /// # Errors
    /// Returns [`CoreError::SignatureInvalid`] when verification fails.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> CoreResult<()> {
        self.0
            .verify(message, signature)
            .map_err(|_| CoreError::SignatureInvalid)
    }
}

/// Owned signing key + derived public key.
#[derive(Debug, Clone)]
pub struct Keypair {
    signing: SigningKey,
}

impl Keypair {
    /// Generate a fresh random keypair using the operating system RNG.
    pub fn generate() -> Self {
        // rand 0.10 dropped `OsRng`; seed the key directly from the OS CSPRNG.
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("operating system RNG unavailable");
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// Reconstruct a keypair from its 32-byte secret seed.
    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(bytes),
        }
    }

    /// Raw secret bytes. Treat as sensitive material.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// Derived public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.signing.verifying_key())
    }

    /// Canonical instance id of this keypair.
    pub fn instance_id(&self) -> InstanceId {
        self.public_key().instance_id()
    }

    /// Sign an arbitrary byte slice.
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing.sign(message)
    }
}

mod verifying_key_bytes {
    use super::VerifyingKey;
    use data_encoding::BASE32_NOPAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(key: &VerifyingKey, s: S) -> Result<S::Ok, S::Error> {
        let encoded = BASE32_NOPAD.encode(key.as_bytes()).to_lowercase();
        s.serialize_str(&encoded)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VerifyingKey, D::Error> {
        let encoded = String::deserialize(d)?;
        let bytes = BASE32_NOPAD
            .decode(encoded.to_uppercase().as_bytes())
            .map_err(serde::de::Error::custom)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| {
            serde::de::Error::custom("public key must be 32 bytes after decoding")
        })?;
        VerifyingKey::from_bytes(&arr).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trip() {
        let kp = Keypair::generate();
        let id = kp.instance_id();
        let parsed = InstanceId::parse(id.as_str()).unwrap();
        assert_eq!(id, parsed);
        assert!(id.as_str().starts_with(ID_PREFIX));
    }

    #[test]
    fn rejects_bad_prefix() {
        assert!(InstanceId::parse("xx:abcd").is_err());
        assert!(InstanceId::parse("").is_err());
    }

    #[test]
    fn sign_and_verify() {
        let kp = Keypair::generate();
        let pk = kp.public_key();
        let sig = kp.sign(b"hello");
        assert!(pk.verify(b"hello", &sig).is_ok());
        assert!(pk.verify(b"world", &sig).is_err());
    }

    #[test]
    fn public_key_serde_round_trip() {
        let kp = Keypair::generate();
        let pk = kp.public_key();
        let json = serde_json::to_string(&pk).unwrap();
        let decoded: PublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.0.as_bytes(), pk.0.as_bytes());
    }
}
