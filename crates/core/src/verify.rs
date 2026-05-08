//! Pure envelope verification.
//!
//! This module contains **no** IO. Anti-replay (which is stateful) is
//! intentionally not part of `verify_envelope`; it is the caller's
//! responsibility to perform a `nonce` lookup against persistent storage and
//! to call [`verify_envelope`] only after — or before, the order does not
//! matter — the lookup itself.

use std::time::Duration;

use time::OffsetDateTime;

use crate::error::{CoreError, CoreResult};
use crate::identity::InstanceId;
use crate::message::SignedMessage;

/// Pluggable clock so tests can inject deterministic time.
pub trait Clock: Send + Sync {
    /// Current UTC time.
    fn now(&self) -> OffsetDateTime;
}

/// Default clock implementation backed by `OffsetDateTime::now_utc`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Verification policy.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    /// Maximum allowed difference between `envelope.timestamp` and `now`.
    /// Architecture spec recommends ±5 minutes.
    pub clock_skew: Duration,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            clock_skew: Duration::from_secs(5 * 60),
        }
    }
}

/// A `SignedMessage` whose signature, recipient and clock have been verified.
#[derive(Debug, Clone)]
pub struct VerifiedEnvelope {
    /// The verified message.
    pub message: SignedMessage,
}

/// Verify the public-key binding, signature, recipient, and clock skew.
///
/// The sender's public key is taken from `msg.sender_public_key`; this
/// function validates that its hash matches `envelope.sender_id`.
///
/// Anti-replay is intentionally not handled here (see module docs).
///
/// # Errors
/// - [`CoreError::RecipientMismatch`] when `recipient_id` ≠ `expected_recipient`.
/// - [`CoreError::TimestampOutOfWindow`] when timestamp is too far from `clock.now()`.
/// - [`CoreError::SignatureInvalid`] when the public-key binding or the signature do not verify.
pub fn verify_envelope(
    msg: SignedMessage,
    expected_recipient: &InstanceId,
    clock: &dyn Clock,
    config: &VerifyConfig,
) -> CoreResult<VerifiedEnvelope> {
    if &msg.envelope.recipient_id != expected_recipient {
        return Err(CoreError::RecipientMismatch {
            expected: expected_recipient.to_string(),
            actual: msg.envelope.recipient_id.to_string(),
        });
    }

    let now = clock.now();
    let skew = (now - msg.envelope.timestamp).abs();
    let skew_std = Duration::from_secs(skew.whole_seconds().unsigned_abs());
    if skew_std > config.clock_skew {
        return Err(CoreError::TimestampOutOfWindow);
    }

    msg.verify_signature()?;

    Ok(VerifiedEnvelope { message: msg })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Keypair;
    use crate::message::{Envelope, ProtocolVerb};
    use serde_json::json;
    use std::sync::Mutex;

    struct FixedClock(Mutex<OffsetDateTime>);

    impl Clock for FixedClock {
        fn now(&self) -> OffsetDateTime {
            *self.0.lock().unwrap()
        }
    }

    fn envelope_at(kp: &Keypair, ts: OffsetDateTime, recipient: InstanceId) -> SignedMessage {
        Envelope {
            sender_id: kp.instance_id(),
            recipient_id: recipient,
            timestamp: ts,
            nonce: "n".into(),
            verb: ProtocolVerb::Ping,
            payload: json!({}),
        }
        .sign(kp)
        .unwrap()
    }

    fn vc() -> VerifyConfig {
        VerifyConfig::default()
    }

    #[test]
    fn accepts_in_window() {
        let kp = Keypair::generate();
        let now = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
        let clock = FixedClock(Mutex::new(now));
        let msg = envelope_at(&kp, now, kp.instance_id());
        let verified = verify_envelope(msg, &kp.instance_id(), &clock, &vc()).unwrap();
        assert_eq!(verified.message.envelope.sender_id, kp.instance_id());
    }

    #[test]
    fn rejects_recipient_mismatch() {
        let kp = Keypair::generate();
        let other = Keypair::generate().instance_id();
        let now = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
        let clock = FixedClock(Mutex::new(now));
        let msg = envelope_at(&kp, now, kp.instance_id());
        let err = verify_envelope(msg, &other, &clock, &vc()).unwrap_err();
        assert!(matches!(err, CoreError::RecipientMismatch { .. }));
    }

    #[test]
    fn rejects_timestamp_out_of_window() {
        let kp = Keypair::generate();
        let now = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
        let past = now - time::Duration::seconds(60 * 60);
        let clock = FixedClock(Mutex::new(now));
        let msg = envelope_at(&kp, past, kp.instance_id());
        let err = verify_envelope(msg, &kp.instance_id(), &clock, &vc()).unwrap_err();
        assert!(matches!(err, CoreError::TimestampOutOfWindow));
    }

    #[test]
    fn rejects_bad_signature() {
        let kp = Keypair::generate();
        let attacker = Keypair::generate();
        let now = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
        let clock = FixedClock(Mutex::new(now));
        let mut msg = envelope_at(&kp, now, kp.instance_id());
        // Forged signature from another key, but pk matches kp ⇒ verify fails.
        let forged = envelope_at(&attacker, now, kp.instance_id());
        msg.signature = forged.signature;
        let err = verify_envelope(msg, &kp.instance_id(), &clock, &vc()).unwrap_err();
        assert!(matches!(err, CoreError::SignatureInvalid));
    }

    #[test]
    fn rejects_pk_id_binding_mismatch() {
        let kp = Keypair::generate();
        let other = Keypair::generate();
        let now = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
        let clock = FixedClock(Mutex::new(now));
        let mut msg = envelope_at(&kp, now, kp.instance_id());
        msg.sender_public_key = other.public_key();
        let err = verify_envelope(msg, &kp.instance_id(), &clock, &vc()).unwrap_err();
        assert!(matches!(err, CoreError::SignatureInvalid));
    }
}
