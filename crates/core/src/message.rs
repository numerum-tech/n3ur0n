//! Wire envelope, signed message, JCS-canonical signing helpers.
//!
//! Every message carries `sender_id, recipient_id, timestamp, nonce, verb,
//! payload, signature`. The signature covers the JCS (RFC 8785) of the
//! envelope (everything but the signature itself).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::identity::{InstanceId, Keypair, PublicKey};

/// The four v0.1 protocol verbs (architecture spec §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolVerb {
    /// Returns the instance descriptor.
    DescribeSelf,
    /// Returns peers from the local directory.
    GetKnownPeers,
    /// Liveness probe.
    Ping,
    /// Invoke a backend capability.
    Invoke,
}

/// Authenticated wire envelope (sans signature).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// Canonical id of the sender.
    pub sender_id: InstanceId,
    /// Canonical id of the intended recipient.
    pub recipient_id: InstanceId,
    /// RFC 3339 UTC timestamp of emission.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Per-message nonce; unique within the anti-replay window.
    pub nonce: String,
    /// Protocol verb.
    pub verb: ProtocolVerb,
    /// Verb-specific payload.
    pub payload: Value,
}

impl Envelope {
    /// JCS bytes covered by the signature.
    ///
    /// # Errors
    /// [`CoreError::Canonical`] if the envelope cannot be canonicalised.
    pub fn canonical_bytes(&self) -> CoreResult<Vec<u8>> {
        serde_jcs::to_vec(self).map_err(|e| CoreError::Canonical(e.to_string()))
    }

    /// Sign this envelope with `keypair`. Verifies the keypair matches
    /// `sender_id` before signing.
    ///
    /// # Errors
    /// [`CoreError::InvalidIdentifier`] if `sender_id` does not match the keypair.
    /// [`CoreError::Canonical`] on JCS failure.
    pub fn sign(self, keypair: &Keypair) -> CoreResult<SignedMessage> {
        if self.sender_id != keypair.instance_id() {
            return Err(CoreError::InvalidIdentifier(format!(
                "envelope sender_id {} does not match keypair {}",
                self.sender_id,
                keypair.instance_id()
            )));
        }
        let bytes = self.canonical_bytes()?;
        let sig = keypair.sign(&bytes);
        Ok(SignedMessage {
            envelope: self,
            sender_public_key: keypair.public_key(),
            signature: signature_codec::encode(&sig),
        })
    }
}

/// An [`Envelope`] together with the sender's public key and signature.
///
/// The public key is transported on the wire so receivers can verify the
/// signature without an external id→key registry. They must additionally
/// check that `hash(sender_public_key)` matches `envelope.sender_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    #[serde(flatten)]
    /// The envelope being signed.
    pub envelope: Envelope,
    /// Sender's Ed25519 public key. Receivers verify
    /// `InstanceId::from_public_key(&sender_public_key.0) == envelope.sender_id`.
    pub sender_public_key: PublicKey,
    /// Hex-lower encoded Ed25519 signature over the envelope's JCS bytes.
    pub signature: String,
}

impl SignedMessage {
    /// Verify both the public key binding (`hash(pk) == sender_id`) and the
    /// signature over the envelope's JCS bytes. Recipient / clock / replay
    /// checks are the caller's responsibility — see
    /// [`crate::verify::verify_envelope`].
    ///
    /// # Errors
    /// - [`CoreError::SignatureInvalid`] on bad public-key binding or bad signature.
    /// - [`CoreError::Crypto`] on decoding errors.
    pub fn verify_signature(&self) -> CoreResult<()> {
        if self.sender_public_key.instance_id() != self.envelope.sender_id {
            return Err(CoreError::SignatureInvalid);
        }
        let bytes = self.envelope.canonical_bytes()?;
        let sig = signature_codec::decode(&self.signature)?;
        self.sender_public_key.verify(&bytes, &sig)
    }
}

mod signature_codec {
    use ed25519_dalek::Signature;

    use crate::error::{CoreError, CoreResult};

    pub(super) fn encode(sig: &Signature) -> String {
        data_encoding::HEXLOWER.encode(&sig.to_bytes())
    }

    pub(super) fn decode(s: &str) -> CoreResult<Signature> {
        let bytes = data_encoding::HEXLOWER
            .decode(s.as_bytes())
            .map_err(|e| CoreError::Crypto(format!("hex decode: {e}")))?;
        if bytes.len() != Signature::BYTE_SIZE {
            return Err(CoreError::Crypto(format!(
                "signature must be {} bytes, got {}",
                Signature::BYTE_SIZE,
                bytes.len()
            )));
        }
        let arr: [u8; Signature::BYTE_SIZE] = bytes.try_into().expect("checked length");
        Ok(Signature::from_bytes(&arr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn sample(kp: &Keypair, payload: Value) -> Envelope {
        Envelope {
            sender_id: kp.instance_id(),
            recipient_id: kp.instance_id(),
            timestamp: now(),
            nonce: "abc123".into(),
            verb: ProtocolVerb::Ping,
            payload,
        }
    }

    #[test]
    fn round_trip_sign_verify() {
        let kp = Keypair::generate();
        let signed = sample(&kp, json!({})).sign(&kp).unwrap();
        signed.verify_signature().unwrap();
    }

    #[test]
    fn tampered_payload_fails() {
        let kp = Keypair::generate();
        let mut signed = sample(&kp, json!({"a": 1})).sign(&kp).unwrap();
        signed.envelope.payload = json!({"a": 2});
        assert!(signed.verify_signature().is_err());
    }

    #[test]
    fn rejects_pk_id_binding_mismatch() {
        let kp = Keypair::generate();
        let other = Keypair::generate();
        let mut signed = sample(&kp, json!({})).sign(&kp).unwrap();
        signed.sender_public_key = other.public_key();
        assert!(signed.verify_signature().is_err());
    }

    #[test]
    fn refuses_signing_with_wrong_keypair() {
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        let env = sample(&kp1, json!({}));
        assert!(env.sign(&kp2).is_err());
    }
}
