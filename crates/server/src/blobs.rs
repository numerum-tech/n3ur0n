//! Publisher blob HTTP handlers (`/n3ur0n/v0/blobs/:hash`).

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use n3ur0n_core::blob::{
    BLOB_TICKET_HEADER, BlobOperation, BlobPurpose, BlobTicketPayload, classify_cap_staging,
    decode_ticket_wire, default_ttl_secs, hash_bytes, validate_hash,
};
use n3ur0n_core::capability::AccessMode;
use n3ur0n_core::message::{Envelope, ProtocolVerb, SignedMessage};
use n3ur0n_core::{Keypair, verify_envelope};
use n3ur0n_storage::blobs::{self, BlobInsert};
use n3ur0n_storage::nonces;
use serde_json::json;
use time::OffsetDateTime;

use crate::http::AppState;

const DEFAULT_PER_PEER_BYTES: i64 = 100 * 1024 * 1024;
const DEFAULT_PER_PEER_BLOBS: i64 = 50;

/// Blob protocol routes (mount under `/n3ur0n/v0` with shared [`AppState`]).
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(
        "/blobs/{*hash}",
        put(put_blob)
            .get(get_blob)
            .head(head_blob)
            .delete(delete_blob),
    )
}

fn blobs_root(config_dir: &Path) -> PathBuf {
    config_dir.join("blobs").join("sha256")
}

fn storage_path(root: &Path, hash: &str) -> PathBuf {
    root.join(hash)
}

fn blob_error(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({"error": msg}))).into_response()
}

async fn verify_ticket(
    state: &AppState,
    headers: &HeaderMap,
    want_op: BlobOperation,
    path_hash: &str,
) -> Result<(SignedMessage, BlobTicketPayload), Response> {
    let raw = headers
        .get(BLOB_TICKET_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| blob_error(StatusCode::UNAUTHORIZED, "missing X-N3UR0N-Ticket"))?;

    let signed =
        decode_ticket_wire(raw).map_err(|e| blob_error(StatusCode::BAD_REQUEST, &e.to_string()))?;

    let verified = verify_envelope(
        signed,
        &state.node.instance_id(),
        state.node.clock().as_ref(),
        &state.node.config().verify,
    )
    .map_err(|e| blob_error(StatusCode::UNAUTHORIZED, &e.to_string()))?;

    let inbound = verified.message;
    if inbound.envelope.verb != ProtocolVerb::BlobTicket {
        return Err(blob_error(
            StatusCode::BAD_REQUEST,
            "ticket verb must be blob_ticket",
        ));
    }

    let now_secs = state.node.clock().now().unix_timestamp();
    let inserted = nonces::insert_if_absent(
        state.node.db(),
        inbound.envelope.sender_id.as_str(),
        &inbound.envelope.nonce,
        now_secs,
    )
    .map_err(|e| blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if !inserted {
        return Err(blob_error(StatusCode::CONFLICT, "ticket replay"));
    }

    let ticket: BlobTicketPayload = serde_json::from_value(inbound.envelope.payload.clone())
        .map_err(|e| blob_error(StatusCode::BAD_REQUEST, &format!("ticket payload: {e}")))?;

    if ticket.operation != want_op {
        return Err(blob_error(
            StatusCode::BAD_REQUEST,
            "ticket operation mismatch",
        ));
    }

    if ticket.expires_at < now_secs {
        return Err(blob_error(StatusCode::UNAUTHORIZED, "ticket expired"));
    }

    if let Some(ref h) = ticket.hash
        && h != path_hash
    {
        return Err(blob_error(StatusCode::BAD_REQUEST, "ticket hash mismatch"));
    }

    Ok((inbound, ticket))
}

fn cap_allows_upload(state: &AppState, cap_name: &str, sender: &str) -> Result<(), Response> {
    let registry = state.node.registry();
    let decl = registry
        .get(cap_name)
        .ok_or_else(|| blob_error(StatusCode::NOT_FOUND, "capability not found"))?;
    match decl.mode {
        AccessMode::Private => Err(blob_error(
            StatusCode::FORBIDDEN,
            "capability refuses upload",
        )),
        AccessMode::Restricted => {
            // v0.1: subscription tokens are out-of-band; accept signed ticket sender.
            let _ = sender;
            Ok(())
        }
        AccessMode::Free => Ok(()),
    }
}

fn check_peer_quota(state: &AppState, uploader: &str, add_bytes: i64) -> Result<(), Response> {
    let now = state.node.clock().now().unix_timestamp();
    let used = blobs::sum_bytes_for_uploader(state.node.db(), uploader, now)
        .map_err(|e| blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if used + add_bytes > DEFAULT_PER_PEER_BYTES {
        return Err(blob_error(StatusCode::TOO_MANY_REQUESTS, "quota exceeded"));
    }
    let count = blobs::count_for_uploader(state.node.db(), uploader, now)
        .map_err(|e| blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if count >= DEFAULT_PER_PEER_BLOBS {
        return Err(blob_error(StatusCode::TOO_MANY_REQUESTS, "quota exceeded"));
    }
    Ok(())
}

fn effective_expires(ticket: &BlobTicketPayload, now: i64) -> i64 {
    if ticket.expires_at > now {
        ticket.expires_at
    } else {
        now + default_ttl_secs(ticket.purpose) as i64
    }
}

#[allow(clippy::too_many_arguments)] // blob-record columns; a struct would just move the args
fn insert_blob_record(
    state: &AppState,
    hash: &str,
    size: i64,
    mime: &str,
    expires_at: i64,
    storage: &Path,
    ticket: &BlobTicketPayload,
    uploader: &str,
    ticket_nonce: &str,
) -> Result<(), Response> {
    let class = classify_cap_staging();
    let whitelist = ticket
        .recipients_whitelist
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());

    let row = BlobInsert {
        hash: hash.to_string(),
        size,
        mime: mime.to_string(),
        expires_at,
        storage_path: storage.display().to_string(),
        provenance: "inbound".into(),
        role: "input".into(),
        anchor_kind: "cap_job".into(),
        processing_status: "staged".into(),
        local_user_id: None,
        client_id: None,
        conversation_id: None,
        dispatch_id: None,
        capability: ticket.capability.clone(),
        remote_sender_id: Some(uploader.to_string()),
        ticket_nonce: Some(ticket_nonce.to_string()),
        invoke_id: None,
        user_visible: class.user_visible,
        user_deletable: class.user_deletable,
        uploader_id: Some(uploader.to_string()),
        recipients_whitelist: whitelist,
    };
    blobs::upsert(state.node.db(), &row)
        .map_err(|e| blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

async fn put_blob(
    State(state): State<AppState>,
    AxumPath(hash): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(config_dir) = state.config_dir.as_deref() else {
        return blob_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "blob storage not configured",
        );
    };
    if let Err(e) = validate_hash(&hash) {
        return blob_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let (signed, ticket) = match verify_ticket(&state, &headers, BlobOperation::Put, &hash).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(cap) = ticket.capability.as_deref() else {
        return blob_error(StatusCode::BAD_REQUEST, "ticket missing capability");
    };
    let Some(size) = ticket.size else {
        return blob_error(StatusCode::BAD_REQUEST, "ticket missing size");
    };
    let Some(mime) = ticket.mime.as_deref() else {
        return blob_error(StatusCode::BAD_REQUEST, "ticket missing mime");
    };

    let uploader = signed.envelope.sender_id.to_string();
    if let Err(r) = cap_allows_upload(&state, cap, &uploader) {
        return r;
    }

    // Idempotent: already stored with same hash.
    if let Ok(Some(existing)) = blobs::get(state.node.db(), &hash) {
        let now = state.node.clock().now().unix_timestamp();
        if existing.expires_at > now {
            return (
                StatusCode::OK,
                axum::Json(json!({
                    "hash": hash,
                    "size": existing.size,
                    "expires_at": OffsetDateTime::from_unix_timestamp(existing.expires_at)
                        .ok()
                        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
                })),
            )
                .into_response();
        }
    }

    if body.len() as u64 != size {
        return blob_error(StatusCode::BAD_REQUEST, "size mismatch");
    }
    let computed = hash_bytes(&body);
    if computed != hash {
        return blob_error(StatusCode::BAD_REQUEST, "hash mismatch");
    }

    if let Err(r) = check_peer_quota(&state, &uploader, size as i64) {
        return r;
    }

    let root = blobs_root(config_dir);
    if let Err(e) = std::fs::create_dir_all(&root) {
        return blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let path = storage_path(&root, &hash);
    if let Err(e) = std::fs::write(&path, &body) {
        return blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    let now = state.node.clock().now().unix_timestamp();
    let expires_at = effective_expires(&ticket, now);
    if let Err(r) = insert_blob_record(
        &state,
        &hash,
        size as i64,
        mime,
        expires_at,
        &path,
        &ticket,
        &uploader,
        &signed.envelope.nonce,
    ) {
        return r;
    }

    (
        StatusCode::CREATED,
        axum::Json(json!({
            "hash": hash,
            "size": size,
            "expires_at": OffsetDateTime::from_unix_timestamp(expires_at)
                .ok()
                .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        })),
    )
        .into_response()
}

fn get_authorization(record: &blobs::BlobRecord, sender: &str) -> Result<(), Response> {
    if record.uploader_id.as_deref() == Some(sender) {
        return Ok(());
    }
    if let Some(ref wl) = record.recipients_whitelist
        && let Ok(ids) = serde_json::from_str::<Vec<String>>(wl)
        && ids.iter().any(|id| id == sender)
    {
        return Ok(());
    }
    Err(blob_error(
        StatusCode::FORBIDDEN,
        "not authorized to download",
    ))
}

async fn get_blob(
    State(state): State<AppState>,
    AxumPath(hash): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = validate_hash(&hash) {
        return blob_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let (signed, _ticket) = match verify_ticket(&state, &headers, BlobOperation::Get, &hash).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let sender = signed.envelope.sender_id.to_string();

    let record = match blobs::get(state.node.db(), &hash) {
        Ok(Some(r)) => r,
        Ok(None) => return blob_error(StatusCode::NOT_FOUND, "blob not found"),
        Err(e) => return blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let now = state.node.clock().now().unix_timestamp();
    if record.expires_at <= now {
        return blob_error(StatusCode::NOT_FOUND, "blob expired");
    }

    if let Err(r) = get_authorization(&record, &sender) {
        return r;
    }

    let path = PathBuf::from(&record.storage_path);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return blob_error(StatusCode::NOT_FOUND, "blob not found"),
    };

    let _ = blobs::touch(state.node.db(), &hash, now);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, record.mime.as_str())
        .header(header::CONTENT_LENGTH, record.size.to_string())
        .header(header::ETAG, format!("\"{}\"", record.hash))
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| blob_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed"))
}

async fn head_blob(State(state): State<AppState>, AxumPath(hash): AxumPath<String>) -> Response {
    if let Err(e) = validate_hash(&hash) {
        return blob_error(StatusCode::BAD_REQUEST, &e.to_string());
    }

    let record = match blobs::get(state.node.db(), &hash) {
        Ok(Some(r)) => r,
        _ => return blob_error(StatusCode::NOT_FOUND, "blob not found"),
    };
    let now = state.node.clock().now().unix_timestamp();
    if record.expires_at <= now {
        return blob_error(StatusCode::NOT_FOUND, "blob not found");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, record.mime.as_str())
        .header(header::CONTENT_LENGTH, record.size.to_string())
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| blob_error(StatusCode::INTERNAL_SERVER_ERROR, "response build failed"))
}

async fn delete_blob(
    State(state): State<AppState>,
    AxumPath(hash): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = validate_hash(&hash) {
        return blob_error(StatusCode::BAD_REQUEST, &e.to_string());
    }
    let (signed, _ticket) =
        match verify_ticket(&state, &headers, BlobOperation::Delete, &hash).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let sender = signed.envelope.sender_id.to_string();

    let record = match blobs::get(state.node.db(), &hash) {
        Ok(Some(r)) => r,
        Ok(None) => return blob_error(StatusCode::NOT_FOUND, "blob not found"),
        Err(e) => return blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    if record.uploader_id.as_deref() != Some(sender.as_str()) {
        return blob_error(StatusCode::FORBIDDEN, "only uploader may delete");
    }

    let _ = std::fs::remove_file(&record.storage_path);
    if let Err(e) = blobs::delete(state.node.db(), &hash) {
        return blob_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Forge a local GET ticket for `/api/v0/files/:hash` download proxy.
pub fn forge_local_get_ticket(
    keypair: &Keypair,
    recipient: n3ur0n_core::InstanceId,
    hash: &str,
    clock: &dyn n3ur0n_core::Clock,
) -> Result<SignedMessage, n3ur0n_core::CoreError> {
    let now = clock.now();
    let expires = now.unix_timestamp() + 300;
    let payload = BlobTicketPayload {
        operation: BlobOperation::Get,
        hash: Some(hash.to_string()),
        size: None,
        mime: None,
        capability: None,
        expires_at: expires,
        purpose: BlobPurpose::Owned,
        requested_ttl_secs: None,
        recipients_whitelist: None,
    };
    let env = Envelope {
        sender_id: keypair.instance_id(),
        recipient_id: recipient,
        timestamp: now,
        nonce: uuid::Uuid::new_v4().to_string(),
        verb: ProtocolVerb::BlobTicket,
        payload: serde_json::to_value(payload)
            .map_err(|e| n3ur0n_core::CoreError::Canonical(e.to_string()))?,
        sender_endpoint: None,
    };
    env.sign(keypair)
}
