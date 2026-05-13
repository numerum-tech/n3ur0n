//! Outbound peer client.
//!
//! All cross-node calls go through this module so that the wire-level
//! mechanics (envelope construction, recipient discovery, reply verification)
//! live in one place. The same client is consumed by the CLI `send`
//! subcommand and by [`crate::discovery`].

use std::time::Duration;

use n3ur0n_core::message::{Envelope, ProtocolVerb, SignedMessage};
use n3ur0n_core::protocol::{
    DescribeSelfRequest, DescribeSelfResponse, GetKnownPeersRequest, GetKnownPeersResponse,
};
use n3ur0n_core::{InstanceId, Keypair};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

// Long timeout: peer invocations may proxy to a slow LLM upstream (Ollama
// queues per-model). 30s was insufficient under modest fan-out; 180s gives
// the upstream room to drain its queue before the client gives up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http transport: {0}")]
    Http(#[from] reqwest::Error),

    #[error("non-success status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid response: {0}")]
    Invalid(String),

    #[error("core: {0}")]
    Core(#[from] n3ur0n_core::CoreError),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Debug, Deserialize)]
struct PublicHealth {
    instance_id: String,
}

/// Build a sane outbound HTTP client. Re-used across requests by callers that
/// want to amortise TLS / connection setup; each helper below also accepts an
/// existing client.
pub fn http_client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("n3ur0n/0.1")
        .build()
        .expect("reqwest::Client builder cannot fail with these defaults")
}

/// GET `<base>/n3ur0n/v0/health` and return the canonical `instance_id`.
pub async fn discover_recipient(client: &Client, base: &str) -> ClientResult<InstanceId> {
    let url = format!("{}/n3ur0n/v0/health", base.trim_end_matches('/'));
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        return Err(ClientError::Status {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    let body: PublicHealth = serde_json::from_slice(&bytes)?;
    Ok(InstanceId::parse(&body.instance_id)?)
}

/// Sign `verb` + `payload` with `keypair`, POST to `base`, verify the reply.
///
/// `sender_endpoint` is the URL at which this caller wants to be reached
/// (typically `node.config().endpoint`). When set, the receiver learns
/// our endpoint passively from the signed envelope and can upsert us
/// into its peer directory (reverse-announce). Pass `None` to remain
/// anonymous (e.g. CLI `send` without a public endpoint).
pub async fn send_signed(
    client: &Client,
    keypair: &Keypair,
    base: &str,
    verb: ProtocolVerb,
    payload: Value,
    sender_endpoint: Option<&str>,
) -> ClientResult<SignedMessage> {
    let recipient = discover_recipient(client, base).await?;
    let env = Envelope {
        sender_id: keypair.instance_id(),
        recipient_id: recipient.clone(),
        timestamp: OffsetDateTime::now_utc(),
        nonce: Uuid::new_v4().to_string(),
        verb,
        payload,
        sender_endpoint: sender_endpoint.map(String::from),
    };
    let signed = env.sign(keypair)?;

    let url = format!("{}/n3ur0n/v0/messages", base.trim_end_matches('/'));
    let resp = client.post(&url).json(&signed).send().await?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        return Err(ClientError::Status {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    let reply: SignedMessage = serde_json::from_slice(&bytes)?;
    reply.verify_signature()?;
    if reply.envelope.recipient_id != keypair.instance_id() {
        return Err(ClientError::Invalid("reply not addressed to us".into()));
    }
    if reply.envelope.sender_id != recipient {
        return Err(ClientError::Invalid(
            "reply sender_id does not match contacted peer".into(),
        ));
    }
    Ok(reply)
}

/// Convenience: signed `describe_self` against `base`.
pub async fn describe_self(
    client: &Client,
    keypair: &Keypair,
    base: &str,
    sender_endpoint: Option<&str>,
) -> ClientResult<DescribeSelfResponse> {
    let payload = serde_json::to_value(DescribeSelfRequest::default())?;
    let reply = send_signed(
        client,
        keypair,
        base,
        ProtocolVerb::DescribeSelf,
        payload,
        sender_endpoint,
    )
    .await?;
    Ok(serde_json::from_value(reply.envelope.payload)?)
}

/// Convenience: signed `get_known_peers` against `base`.
pub async fn get_known_peers(
    client: &Client,
    keypair: &Keypair,
    base: &str,
    req: GetKnownPeersRequest,
    sender_endpoint: Option<&str>,
) -> ClientResult<GetKnownPeersResponse> {
    let payload = serde_json::to_value(req)?;
    let reply = send_signed(
        client,
        keypair,
        base,
        ProtocolVerb::GetKnownPeers,
        payload,
        sender_endpoint,
    )
    .await?;
    Ok(serde_json::from_value(reply.envelope.payload)?)
}
