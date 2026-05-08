//! Protocol message envelope.
//!
//! Every message carries `sender_id, recipient_id, timestamp, nonce, payload, signature`.
//! Signature covers JCS (RFC 8785) of the canonical concatenation of the first five fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::identity::{InstanceId, Keypair, PublicKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolVerb {
    DescribeSelf,
    GetKnownPeers,
    Ping,
    Invoke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub sender_id: InstanceId,
    pub recipient_id: InstanceId,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub nonce: String,
    pub verb: ProtocolVerb,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub signature: String,
}

impl Envelope {
    /// Canonical bytes signed by sender.
    pub fn canonical_bytes(&self) -> CoreResult<Vec<u8>> {
        serde_jcs::to_vec(self).map_err(|e| CoreError::Canonical(e.to_string()))
    }

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
            signature: hex::encode_signature(&sig),
        })
    }
}

impl SignedMessage {
    /// Verify signature only. Recipient/timestamp/nonce checks are caller responsibility.
    pub fn verify_signature(&self, sender_pk: &PublicKey) -> CoreResult<()> {
        let bytes = self.envelope.canonical_bytes()?;
        let sig = hex::decode_signature(&self.signature)?;
        sender_pk.verify(&bytes, &sig)
    }
}

mod hex {
    use ed25519_dalek::Signature;

    use crate::error::{CoreError, CoreResult};

    pub fn encode_signature(sig: &Signature) -> String {
        data_encoding::HEXLOWER.encode(&sig.to_bytes())
    }

    pub fn decode_signature(s: &str) -> CoreResult<Signature> {
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

    #[test]
    fn round_trip_sign_verify() {
        let kp = Keypair::generate();
        let env = Envelope {
            sender_id: kp.instance_id(),
            recipient_id: kp.instance_id(),
            timestamp: now(),
            nonce: "abc123".into(),
            verb: ProtocolVerb::Ping,
            payload: json!({}),
        };
        let signed = env.sign(&kp).unwrap();
        signed.verify_signature(&kp.public_key()).unwrap();
    }

    #[test]
    fn tampered_payload_fails() {
        let kp = Keypair::generate();
        let env = Envelope {
            sender_id: kp.instance_id(),
            recipient_id: kp.instance_id(),
            timestamp: now(),
            nonce: "abc123".into(),
            verb: ProtocolVerb::Ping,
            payload: json!({"a": 1}),
        };
        let mut signed = env.sign(&kp).unwrap();
        signed.envelope.payload = json!({"a": 2});
        assert!(signed.verify_signature(&kp.public_key()).is_err());
    }
}
