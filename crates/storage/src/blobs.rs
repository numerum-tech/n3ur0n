//! Blob index repository (SQLite).

use serde_json::Value;
use time::OffsetDateTime;

use crate::{Db, StorageResult};

/// Row in the `blobs` table.
#[derive(Debug, Clone)]
pub struct BlobRecord {
    pub hash: String,
    pub size: i64,
    pub mime: String,
    pub expires_at: i64,
    pub storage_path: String,
    pub provenance: String,
    pub role: String,
    pub anchor_kind: String,
    pub processing_status: String,
    pub local_user_id: Option<i64>,
    pub client_id: Option<String>,
    pub conversation_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub capability: Option<String>,
    pub remote_sender_id: Option<String>,
    pub ticket_nonce: Option<String>,
    pub invoke_id: Option<String>,
    pub user_visible: bool,
    pub user_deletable: bool,
    pub uploader_id: Option<String>,
    pub recipients_whitelist: Option<String>,
    pub created_at: i64,
    pub last_access_at: Option<i64>,
}

/// Insert parameters for a new blob index entry.
#[derive(Debug, Clone)]
pub struct BlobInsert {
    pub hash: String,
    pub size: i64,
    pub mime: String,
    pub expires_at: i64,
    pub storage_path: String,
    pub provenance: String,
    pub role: String,
    pub anchor_kind: String,
    pub processing_status: String,
    pub local_user_id: Option<i64>,
    pub client_id: Option<String>,
    pub conversation_id: Option<String>,
    pub dispatch_id: Option<String>,
    pub capability: Option<String>,
    pub remote_sender_id: Option<String>,
    pub ticket_nonce: Option<String>,
    pub invoke_id: Option<String>,
    pub user_visible: bool,
    pub user_deletable: bool,
    pub uploader_id: Option<String>,
    pub recipients_whitelist: Option<String>,
}

fn row_from_query(row: &rusqlite::Row<'_>) -> rusqlite::Result<BlobRecord> {
    Ok(BlobRecord {
        hash: row.get(0)?,
        size: row.get(1)?,
        mime: row.get(2)?,
        expires_at: row.get(3)?,
        storage_path: row.get(4)?,
        provenance: row.get(5)?,
        role: row.get(6)?,
        anchor_kind: row.get(7)?,
        processing_status: row.get(8)?,
        local_user_id: row.get(9)?,
        client_id: row.get(10)?,
        conversation_id: row.get(11)?,
        dispatch_id: row.get(12)?,
        capability: row.get(13)?,
        remote_sender_id: row.get(14)?,
        ticket_nonce: row.get(15)?,
        invoke_id: row.get(16)?,
        user_visible: row.get::<_, i64>(17)? != 0,
        user_deletable: row.get::<_, i64>(18)? != 0,
        uploader_id: row.get(19)?,
        recipients_whitelist: row.get(20)?,
        created_at: row.get(21)?,
        last_access_at: row.get(22)?,
    })
}

const SELECT_COLS: &str = "\
    hash, size, mime, expires_at, storage_path,
    provenance, role, anchor_kind, processing_status,
    local_user_id, client_id, conversation_id, dispatch_id,
    capability, remote_sender_id, ticket_nonce, invoke_id,
    user_visible, user_deletable, uploader_id, recipients_whitelist,
    created_at, last_access_at";

/// Insert or replace a blob record.
pub fn upsert(pool: &Db, row: &BlobInsert) -> StorageResult<()> {
    let conn = pool.get()?;
    conn.execute(
        "INSERT INTO blobs (
            hash, size, mime, expires_at, storage_path,
            provenance, role, anchor_kind, processing_status,
            local_user_id, client_id, conversation_id, dispatch_id,
            capability, remote_sender_id, ticket_nonce, invoke_id,
            user_visible, user_deletable, uploader_id, recipients_whitelist
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8, ?9,
            ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17,
            ?18, ?19, ?20, ?21
        )
        ON CONFLICT(hash) DO UPDATE SET
            size = excluded.size,
            mime = excluded.mime,
            expires_at = excluded.expires_at,
            storage_path = excluded.storage_path,
            processing_status = excluded.processing_status,
            last_access_at = strftime('%s', 'now')",
        rusqlite::params![
            row.hash,
            row.size,
            row.mime,
            row.expires_at,
            row.storage_path,
            row.provenance,
            row.role,
            row.anchor_kind,
            row.processing_status,
            row.local_user_id,
            row.client_id,
            row.conversation_id,
            row.dispatch_id,
            row.capability,
            row.remote_sender_id,
            row.ticket_nonce,
            row.invoke_id,
            if row.user_visible { 1 } else { 0 },
            if row.user_deletable { 1 } else { 0 },
            row.uploader_id,
            row.recipients_whitelist,
        ],
    )?;
    Ok(())
}

/// Fetch a blob by hash.
pub fn get(pool: &Db, hash: &str) -> StorageResult<Option<BlobRecord>> {
    let conn = pool.get()?;
    let sql = format!("SELECT {SELECT_COLS} FROM blobs WHERE hash = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([hash])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(row_from_query(row)?));
    }
    Ok(None)
}

/// Delete a blob record by hash.
pub fn delete(pool: &Db, hash: &str) -> StorageResult<bool> {
    let conn = pool.get()?;
    let n = conn.execute("DELETE FROM blobs WHERE hash = ?1", [hash])?;
    Ok(n > 0)
}

/// List user-visible blobs for a local user or anonymous client.
pub fn list_user_visible(
    pool: &Db,
    local_user_id: Option<i64>,
    client_id: Option<&str>,
    limit: i64,
) -> StorageResult<Vec<BlobRecord>> {
    let conn = pool.get()?;
    let sql = format!(
        "SELECT {SELECT_COLS} FROM blobs
         WHERE user_visible = 1
           AND (
             (?1 IS NOT NULL AND local_user_id = ?1)
             OR (?2 IS NOT NULL AND client_id = ?2)
           )
         ORDER BY created_at DESC
         LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![local_user_id, client_id, limit],
        row_from_query,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// List cap-job staging blobs (class C).
pub fn list_cap_jobs(pool: &Db, limit: i64) -> StorageResult<Vec<BlobRecord>> {
    let conn = pool.get()?;
    let sql = format!(
        "SELECT {SELECT_COLS} FROM blobs
         WHERE anchor_kind = 'cap_job'
         ORDER BY created_at DESC
         LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([limit], row_from_query)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Sum of active blob sizes for a remote uploader.
pub fn sum_bytes_for_uploader(pool: &Db, uploader_id: &str, now: i64) -> StorageResult<i64> {
    let conn = pool.get()?;
    let v: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM blobs
         WHERE uploader_id = ?1 AND expires_at > ?2",
        rusqlite::params![uploader_id, now],
        |r| r.get(0),
    )?;
    Ok(v)
}

/// Count active blobs for a remote uploader.
pub fn count_for_uploader(pool: &Db, uploader_id: &str, now: i64) -> StorageResult<i64> {
    let conn = pool.get()?;
    let v: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blobs
         WHERE uploader_id = ?1 AND expires_at > ?2",
        rusqlite::params![uploader_id, now],
        |r| r.get(0),
    )?;
    Ok(v)
}

/// Delete expired blob records; returns deleted rows for filesystem cleanup.
pub fn delete_expired(pool: &Db, now: i64) -> StorageResult<Vec<BlobRecord>> {
    let conn = pool.get()?;
    let sql = format!("SELECT {SELECT_COLS} FROM blobs WHERE expires_at <= ?1");
    let mut stmt = conn.prepare(&sql)?;
    let expired: Vec<BlobRecord> = stmt
        .query_map([now], row_from_query)?
        .collect::<Result<Vec<_>, _>>()?;
    if !expired.is_empty() {
        conn.execute("DELETE FROM blobs WHERE expires_at <= ?1", [now])?;
    }
    Ok(expired)
}

/// Touch last-access time and optionally extend TTL for input blobs.
pub fn touch(pool: &Db, hash: &str, now: i64) -> StorageResult<()> {
    let conn = pool.get()?;
    conn.execute(
        "UPDATE blobs SET last_access_at = ?2 WHERE hash = ?1",
        rusqlite::params![hash, now],
    )?;
    Ok(())
}

/// JSON summary for API responses.
pub fn record_to_json(rec: &BlobRecord) -> Value {
    serde_json::json!({
        "hash": rec.hash,
        "size": rec.size,
        "mime": rec.mime,
        "expires_at": OffsetDateTime::from_unix_timestamp(rec.expires_at)
            .ok()
            .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        "provenance": rec.provenance,
        "role": rec.role,
        "anchor_kind": rec.anchor_kind,
        "processing_status": rec.processing_status,
        "user_visible": rec.user_visible,
        "user_deletable": rec.user_deletable,
        "capability": rec.capability,
        "remote_sender_id": rec.remote_sender_id,
        "created_at": rec.created_at,
    })
}
