//! Cryptographic identity for an n3ur0n instance.
//!
//! Identifier: `n3:` + Base32(SHA-256(Ed25519 public key)) RFC4648 no padding.
//! Auto-verifiable; no registry required.

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

const ID_PREFIX: &str = "n3:";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn from_public_key(pk: &VerifyingKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(pk.as_bytes());
        let digest = hasher.finalize();
        let encoded = BASE32_NOPAD.encode(&digest).to_lowercase();
        Self(format!("{ID_PREFIX}{encoded}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKey(#[serde(with = "verifying_key_bytes")] pub VerifyingKey);

impl PublicKey {
    pub fn instance_id(&self) -> InstanceId {
        InstanceId::from_public_key(&self.0)
    }

    pub fn verify(&self, message: &[u8], signature: &Signature) -> CoreResult<()> {
        self.0
            .verify(message, signature)
            .map_err(|_| CoreError::SignatureInvalid)
    }
}

pub struct Keypair {
    signing: SigningKey,
}

impl Keypair {
    pub fn generate() -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        Self { signing }
    }

    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(bytes),
        }
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.signing.verifying_key())
    }

    pub fn instance_id(&self) -> InstanceId {
        self.public_key().instance_id()
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing.sign(message)
    }
}

mod verifying_key_bytes {
    use super::VerifyingKey;
    use data_encoding::BASE32_NOPAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &VerifyingKey, s: S) -> Result<S::Ok, S::Error> {
        let encoded = BASE32_NOPAD.encode(key.as_bytes()).to_lowercase();
        s.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VerifyingKey, D::Error> {
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
}
