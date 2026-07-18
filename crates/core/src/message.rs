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

/// Protocol verbs. The four v0.1 verbs plus the blob-layer `blob_ticket`
/// (authorized only on `/n3ur0n/v0/blobs`, not on `/messages`).
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
    /// Authorizes a blob HTTP operation (PUT/GET/DELETE). Not dispatched
    /// via the `/messages` handler.
    BlobTicket,
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
    /// Optional reverse-announce: the URL the sender wants to be reached
    /// at. Receivers MAY upsert `(sender_id, sender_endpoint)` into their
    /// peer directory after signature verification, enabling passive
    /// reverse discovery (when A calls B, B learns A's endpoint).
    ///
    /// Signed: yes — included in the JCS bytes covered by the signature.
    /// The receiver SHOULD treat the endpoint as a *claim* (verify by
    /// pulling `describe_self` from it and matching the returned id) for
    /// strong correctness, or simply cache TOFU-style. v0.3 ships TOFU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_endpoint: Option<String>,
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
            sender_endpoint: None,
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

    /// A fully deterministic envelope exercising the JCS edge cases that
    /// matter to the signature: out-of-order object keys, nested
    /// object/array, float vs integer number formatting, unicode escaping,
    /// booleans and null. Built from parsed ids + a fixed timestamp so it
    /// carries no random or clock state.
    fn golden_envelope() -> Envelope {
        Envelope {
            sender_id: InstanceId::parse("n3:eytey6vdqdeoodtcpx3ugxsqyy3dxpjwp2dtgdjoauggdvjnyuaa")
                .unwrap(),
            recipient_id: InstanceId::parse(
                "n3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            timestamp: OffsetDateTime::parse(
                "2026-01-02T03:04:05Z",
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap(),
            nonce: "nonce-Ω-123".into(),
            verb: ProtocolVerb::Invoke,
            payload: json!({
                "z": 1,
                "a": { "nested": true, "list": [3, 2, 1], "flt": 1.5 },
                "unicode": "héllo→",
                "int": 42,
                "empty": null,
                "big": 1000000
            }),
            sender_endpoint: Some("https://seed.example/n3ur0n/v0".into()),
        }
    }

    /// Golden JCS vector. The signature covers exactly these bytes, so this
    /// string MUST NOT change across `serde_jcs` upgrades — a diff here means
    /// canonicalization drifted and every signature on the network would
    /// silently diverge. Regenerate only with a deliberate protocol bump.
    const GOLDEN_JCS: &str = r#"{"nonce":"nonce-Ω-123","payload":{"a":{"flt":1.5,"list":[3,2,1],"nested":true},"big":1000000,"empty":null,"int":42,"unicode":"héllo→","z":1},"recipient_id":"n3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sender_endpoint":"https://seed.example/n3ur0n/v0","sender_id":"n3:eytey6vdqdeoodtcpx3ugxsqyy3dxpjwp2dtgdjoauggdvjnyuaa","timestamp":"2026-01-02T03:04:05Z","verb":"invoke"}"#;

    #[test]
    fn jcs_canonicalization_is_stable() {
        let bytes = golden_envelope().canonical_bytes().unwrap();
        let got = String::from_utf8(bytes).expect("JCS output is valid UTF-8");
        assert_eq!(
            got, GOLDEN_JCS,
            "serde_jcs canonical output drifted — signatures would diverge network-wide"
        );
    }
}
