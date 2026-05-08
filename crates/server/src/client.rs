//! Outbound client: build a signed envelope, POST it to a remote n3ur0n
//! `/n3ur0n/v0/messages` endpoint, verify the signed reply.

use std::time::Duration;

use anyhow::{Context, Result};
use n3ur0n_core::message::{Envelope, ProtocolVerb, SignedMessage};
use n3ur0n_core::{InstanceId, Keypair};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct PublicHealth {
    instance_id: String,
}

/// Discover a peer's canonical instance id via its public health endpoint.
pub async fn discover_recipient(client: &Client, base: &str) -> Result<InstanceId> {
    let url = format!("{}/n3ur0n/v0/health", base.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?;
    let body: PublicHealth = resp.json().await?;
    InstanceId::parse(&body.instance_id).map_err(|e| anyhow::anyhow!(e))
}

/// Send `verb` with `payload` to `base` (e.g. `http://node-b:4242`).
/// Returns the verified reply envelope.
pub async fn send_signed(
    keypair: &Keypair,
    base: &str,
    verb: ProtocolVerb,
    payload: Value,
) -> Result<SignedMessage> {
    let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
    let recipient = discover_recipient(&client, base).await?;

    let env = Envelope {
        sender_id: keypair.instance_id(),
        recipient_id: recipient.clone(),
        timestamp: OffsetDateTime::now_utc(),
        nonce: Uuid::new_v4().to_string(),
        verb,
        payload,
    };
    let signed = env.sign(keypair)?;

    let url = format!("{}/n3ur0n/v0/messages", base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&signed)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        anyhow::bail!("remote returned {status}: {body}");
    }
    let reply: SignedMessage = serde_json::from_slice(&bytes)?;
    reply
        .verify_signature()
        .context("reply signature failed to verify")?;
    if reply.envelope.recipient_id != keypair.instance_id() {
        anyhow::bail!("reply not addressed to us");
    }
    if reply.envelope.sender_id != recipient {
        anyhow::bail!("reply sender_id does not match contacted peer");
    }
    Ok(reply)
}
