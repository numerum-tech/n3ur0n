//! Outbound blob upload/download helpers.

use std::time::Duration;

use n3ur0n_core::blob::{
    decode_ticket_wire, encode_ticket_wire, hash_bytes, BlobOperation, BlobPurpose, BlobRef,
    BlobTicketPayload, BLOB_TICKET_HEADER,
};
use n3ur0n_core::message::{Envelope, ProtocolVerb};
use n3ur0n_core::{InstanceId, Keypair};
use reqwest::Client;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::client::{http_client, ClientError, ClientResult};

const TICKET_TTL_SECS: i64 = 300;

/// Build a signed PUT ticket for uploading a blob to a remote publisher.
pub fn forge_put_ticket(
    keypair: &Keypair,
    recipient: &InstanceId,
    hash: &str,
    size: u64,
    mime: &str,
    capability: &str,
) -> ClientResult<n3ur0n_core::SignedMessage> {
    let now = OffsetDateTime::now_utc();
    let expires = now.unix_timestamp() + TICKET_TTL_SECS;
    let payload = BlobTicketPayload {
        operation: BlobOperation::Put,
        hash: Some(hash.to_string()),
        size: Some(size),
        mime: Some(mime.to_string()),
        capability: Some(capability.to_string()),
        expires_at: expires,
        purpose: BlobPurpose::Input,
        requested_ttl_secs: None,
        recipients_whitelist: None,
    };
    let env = Envelope {
        sender_id: keypair.instance_id(),
        recipient_id: recipient.clone(),
        timestamp: now,
        nonce: Uuid::new_v4().to_string(),
        verb: ProtocolVerb::BlobTicket,
        payload: serde_json::to_value(payload)?,
        sender_endpoint: None,
    };
    Ok(env.sign(keypair)?)
}

/// Build a signed GET ticket for downloading a blob.
pub fn forge_get_ticket(
    keypair: &Keypair,
    recipient: &InstanceId,
    hash: &str,
) -> ClientResult<n3ur0n_core::SignedMessage> {
    let now = OffsetDateTime::now_utc();
    let expires = now.unix_timestamp() + TICKET_TTL_SECS;
    let payload = BlobTicketPayload {
        operation: BlobOperation::Get,
        hash: Some(hash.to_string()),
        size: None,
        mime: None,
        capability: None,
        expires_at: expires,
        purpose: BlobPurpose::Output,
        requested_ttl_secs: None,
        recipients_whitelist: None,
    };
    let env = Envelope {
        sender_id: keypair.instance_id(),
        recipient_id: recipient.clone(),
        timestamp: now,
        nonce: Uuid::new_v4().to_string(),
        verb: ProtocolVerb::BlobTicket,
        payload: serde_json::to_value(payload)?,
        sender_endpoint: None,
    };
    Ok(env.sign(keypair)?)
}

#[derive(Debug, Deserialize)]
struct PutBlobResponse {
    hash: String,
    size: u64,
}

/// HEAD check whether a blob exists on a remote publisher.
pub async fn head_blob(client: &Client, base: &str, hash: &str) -> ClientResult<bool> {
    let url = blob_url(base, hash);
    let resp = client.head(&url).send().await?;
    Ok(resp.status().is_success())
}

/// Upload bytes to a remote publisher (idempotent if already present).
pub async fn upload_blob(
    client: &Client,
    keypair: &Keypair,
    base: &str,
    recipient: &InstanceId,
    capability: &str,
    mime: &str,
    bytes: &[u8],
) -> ClientResult<BlobRef> {
    let hash = hash_bytes(bytes);
    if head_blob(client, base, &hash).await? {
        return Ok(BlobRef {
            hash: hash.clone(),
            size: bytes.len() as u64,
            mime: mime.to_string(),
            fetch_url: None,
        });
    }

    let ticket = forge_put_ticket(keypair, recipient, &hash, bytes.len() as u64, mime, capability)?;
    let header = encode_ticket_wire(&ticket)?;
    let url = blob_url(base, &hash);
    let resp = client
        .put(&url)
        .header(BLOB_TICKET_HEADER, header)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes.to_vec())
        .timeout(Duration::from_secs(600))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if !status.is_success() {
        return Err(ClientError::Status {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    let _: PutBlobResponse = serde_json::from_slice(&body)?;
    Ok(BlobRef {
        hash,
        size: bytes.len() as u64,
        mime: mime.to_string(),
        fetch_url: None,
    })
}

/// Download a blob from a remote publisher.
pub async fn download_blob(
    client: &Client,
    keypair: &Keypair,
    base: &str,
    recipient: &InstanceId,
    blob_ref: &BlobRef,
) -> ClientResult<Vec<u8>> {
    let ticket = forge_get_ticket(keypair, recipient, &blob_ref.hash)?;
    let header = encode_ticket_wire(&ticket)?;
    let url = blob_url(base, &blob_ref.hash);
    let resp = client
        .get(&url)
        .header(BLOB_TICKET_HEADER, header)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if !status.is_success() {
        return Err(ClientError::Status {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    Ok(body.to_vec())
}

fn blob_url(base: &str, hash: &str) -> String {
    format!(
        "{}/n3ur0n/v0/blobs/{}",
        base.trim_end_matches('/'),
        urlencoding_encode(hash)
    )
}

fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other if other.is_ascii_alphanumeric() || "-_.~".contains(other) => other.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

/// Convenience: upload using a fresh HTTP client + recipient discovery.
pub async fn upload_blob_discover(
    keypair: &Keypair,
    base: &str,
    capability: &str,
    mime: &str,
    bytes: &[u8],
) -> ClientResult<BlobRef> {
    let client = http_client();
    let recipient = crate::client::discover_recipient(&client, base).await?;
    upload_blob(&client, keypair, base, &recipient, capability, mime, bytes).await
}

#[allow(dead_code)]
fn parse_ticket_header(raw: &str) -> ClientResult<n3ur0n_core::SignedMessage> {
    decode_ticket_wire(raw).map_err(ClientError::Core)
}
