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
    /// Human-readable local path. Not unique, not an identifier.
    pub path: Option<String>,
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
    /// Human-readable local path, already sanitized by the caller.
    pub path: Option<String>,
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
        path: row.get(3)?,
        expires_at: row.get(4)?,
        storage_path: row.get(5)?,
        provenance: row.get(6)?,
        role: row.get(7)?,
        anchor_kind: row.get(8)?,
        processing_status: row.get(9)?,
        local_user_id: row.get(10)?,
        client_id: row.get(11)?,
        conversation_id: row.get(12)?,
        dispatch_id: row.get(13)?,
        capability: row.get(14)?,
        remote_sender_id: row.get(15)?,
        ticket_nonce: row.get(16)?,
        invoke_id: row.get(17)?,
        user_visible: row.get::<_, i64>(18)? != 0,
        user_deletable: row.get::<_, i64>(19)? != 0,
        uploader_id: row.get(20)?,
        recipients_whitelist: row.get(21)?,
        created_at: row.get(22)?,
        last_access_at: row.get(23)?,
    })
}

const SELECT_COLS: &str = "\
    hash, size, mime, path, expires_at, storage_path,
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
            hash, size, mime, path, expires_at, storage_path,
            provenance, role, anchor_kind, processing_status,
            local_user_id, client_id, conversation_id, dispatch_id,
            capability, remote_sender_id, ticket_nonce, invoke_id,
            user_visible, user_deletable, uploader_id, recipients_whitelist
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18,
            ?19, ?20, ?21, ?22
        )
        ON CONFLICT(hash) DO UPDATE SET
            size = excluded.size,
            mime = excluded.mime,
            -- same bytes re-uploaded under a new name: keep the first path we
            -- were given rather than letting a later upload rewrite it.
            path = COALESCE(blobs.path, excluded.path),
            expires_at = excluded.expires_at,
            storage_path = excluded.storage_path,
            processing_status = excluded.processing_status,
            last_access_at = strftime('%s', 'now')",
        rusqlite::params![
            row.hash,
            row.size,
            row.mime,
            row.path,
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
/// Promote a staged local-cache blob (class D) to an outbound upload (class A)
/// once its bytes have actually been `PUT` at a peer.
///
/// [`upsert`] deliberately never rewrites a row's classification — that is what
/// stops a later write from relabelling an inbound result (B) or a cap staging
/// blob (C) — so the transition the spec describes (D is "not yet referenced on
/// the network") needs its own statement. The `WHERE` clause is the guard:
/// `outbound` + `input` matches only classes A and D, so B (`inbound`/`output`)
/// and C (`cap_job`) can never be caught by it. Re-sending a file that is
/// already class A refreshes its expiry and changes nothing else.
///
/// Returns whether a row was promoted.
pub fn mark_outbound(pool: &Db, hash: &str, expires_at: i64) -> StorageResult<bool> {
    let conn = pool.get()?;
    let n = conn.execute(
        "UPDATE blobs SET
            anchor_kind = 'user_session',
            processing_status = 'referenced',
            user_visible = 1,
            user_deletable = 1,
            expires_at = ?2,
            last_access_at = strftime('%s', 'now')
         WHERE hash = ?1
           AND provenance = 'outbound'
           AND role = 'input'
           AND anchor_kind IN ('local_cache', 'user_session')",
        rusqlite::params![hash, expires_at],
    )?;
    Ok(n > 0)
}

/// Record that these bytes came back as the output of a remote capability
/// (class B), optionally under a name the capability chose.
///
/// The sibling of [`mark_outbound`], and needed for the same reason: [`upsert`]
/// never rewrites a classification. It matters most when a capability returns
/// the *same* bytes it was given — a rename — because the row already exists as
/// the class A blob we sent, and nothing else would move it to B.
///
/// `name` replaces the stored path when set. That is the one place the
/// "first name wins" rule of [`upsert`] is deliberately overridden: there, a
/// second name means the same bytes were re-uploaded under a different
/// filename and the original should stand; here it is a capability's answer to
/// a request to rename, which is the whole point of the call.
///
/// The `WHERE` clause spares class C: a cap staging blob belongs to the peer
/// that uploaded it, and must never be relabelled as one of our results.
pub fn mark_inbound_output(
    pool: &Db,
    hash: &str,
    name: Option<&str>,
    expires_at: i64,
) -> StorageResult<bool> {
    let conn = pool.get()?;
    let n = conn.execute(
        "UPDATE blobs SET
            provenance = 'inbound',
            role = 'output',
            anchor_kind = 'user_session',
            processing_status = 'ready',
            user_visible = 1,
            user_deletable = 1,
            path = COALESCE(?2, path),
            expires_at = ?3,
            last_access_at = strftime('%s', 'now')
         WHERE hash = ?1
           AND anchor_kind IN ('local_cache', 'user_session')",
        rusqlite::params![hash, name, expires_at],
    )?;
    Ok(n > 0)
}

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
        "path": rec.path,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(hash: &str, client_id: Option<&str>, path: Option<&str>) -> BlobInsert {
        BlobInsert {
            hash: hash.into(),
            size: 3,
            mime: "text/plain".into(),
            path: path.map(Into::into),
            expires_at: 4_102_444_800,
            storage_path: format!("/tmp/{hash}"),
            provenance: "inbound".into(),
            role: "output".into(),
            anchor_kind: "user_session".into(),
            processing_status: "ready".into(),
            local_user_id: None,
            client_id: client_id.map(Into::into),
            conversation_id: None,
            dispatch_id: None,
            capability: Some("translate".into()),
            remote_sender_id: None,
            ticket_nonce: None,
            invoke_id: None,
            user_visible: true,
            user_deletable: true,
            uploader_id: None,
            recipients_whitelist: None,
        }
    }

    fn row_classed(hash: &str, provenance: &str, role: &str, anchor: &str) -> BlobInsert {
        let mut r = row(hash, Some("client-a"), None);
        r.provenance = provenance.into();
        r.role = role.into();
        r.anchor_kind = anchor.into();
        r.processing_status = "staged".into();
        r
    }

    #[test]
    fn mark_outbound_promotes_local_cache_and_spares_every_other_class() {
        let db = crate::open_in_memory().unwrap();
        upsert(
            &db,
            &row_classed("sha256:d", "outbound", "input", "local_cache"),
        )
        .unwrap();
        upsert(
            &db,
            &row_classed("sha256:b", "inbound", "output", "user_session"),
        )
        .unwrap();
        upsert(&db, &row_classed("sha256:c", "inbound", "input", "cap_job")).unwrap();

        assert!(mark_outbound(&db, "sha256:d", 9_000).unwrap());
        let d = get(&db, "sha256:d").unwrap().unwrap();
        assert_eq!(d.anchor_kind, "user_session");
        assert_eq!(d.processing_status, "referenced");
        assert_eq!(d.expires_at, 9_000);

        // A class B result and a class C staging blob must survive untouched:
        // relabelling either would change who may see or delete it.
        assert!(!mark_outbound(&db, "sha256:b", 9_000).unwrap());
        let b = get(&db, "sha256:b").unwrap().unwrap();
        assert_eq!(b.anchor_kind, "user_session");
        assert_eq!(b.role, "output");
        assert_eq!(b.processing_status, "staged");

        assert!(!mark_outbound(&db, "sha256:c", 9_000).unwrap());
        let c = get(&db, "sha256:c").unwrap().unwrap();
        assert_eq!(c.anchor_kind, "cap_job");
        assert_eq!(c.processing_status, "staged");
    }

    #[test]
    fn mark_outbound_on_an_already_outbound_blob_only_refreshes_expiry() {
        let db = crate::open_in_memory().unwrap();
        upsert(
            &db,
            &row_classed("sha256:a", "outbound", "input", "user_session"),
        )
        .unwrap();
        assert!(mark_outbound(&db, "sha256:a", 12_345).unwrap());
        let a = get(&db, "sha256:a").unwrap().unwrap();
        assert_eq!(a.anchor_kind, "user_session");
        assert_eq!(a.expires_at, 12_345);
    }

    #[test]
    fn mark_inbound_output_moves_a_sent_file_to_class_b_under_its_new_name() {
        let db = crate::open_in_memory().unwrap();
        let mut sent = row_classed("sha256:sent", "outbound", "input", "user_session");
        sent.path = Some("note.txt".into());
        upsert(&db, &sent).unwrap();

        // A rename returns the same bytes, so this is the very row we sent.
        assert!(mark_inbound_output(&db, "sha256:sent", Some("renamed.txt"), 9_000).unwrap());
        let r = get(&db, "sha256:sent").unwrap().unwrap();
        assert_eq!(r.provenance, "inbound");
        assert_eq!(r.role, "output");
        assert_eq!(r.path.as_deref(), Some("renamed.txt"));
        assert_eq!(r.processing_status, "ready");
    }

    #[test]
    fn mark_inbound_output_keeps_the_name_when_the_cap_supplies_none() {
        let db = crate::open_in_memory().unwrap();
        let mut sent = row_classed("sha256:keep", "outbound", "input", "local_cache");
        sent.path = Some("note.txt".into());
        upsert(&db, &sent).unwrap();
        assert!(mark_inbound_output(&db, "sha256:keep", None, 9_000).unwrap());
        assert_eq!(
            get(&db, "sha256:keep").unwrap().unwrap().path.as_deref(),
            Some("note.txt")
        );
    }

    #[test]
    fn mark_inbound_output_never_touches_cap_staging() {
        let db = crate::open_in_memory().unwrap();
        upsert(
            &db,
            &row_classed("sha256:capjob", "inbound", "input", "cap_job"),
        )
        .unwrap();
        assert!(!mark_inbound_output(&db, "sha256:capjob", Some("mine.txt"), 9_000).unwrap());
        let r = get(&db, "sha256:capjob").unwrap().unwrap();
        assert_eq!(r.anchor_kind, "cap_job");
        assert_eq!(r.role, "input");
    }

    #[test]
    fn mark_outbound_reports_a_missing_blob() {
        let db = crate::open_in_memory().unwrap();
        assert!(!mark_outbound(&db, "sha256:nope", 9_000).unwrap());
    }

    /// The Files panel selects on `local_user_id = ?1 OR client_id = ?2`, so a
    /// user_visible blob carrying neither is unreachable: it exists, counts
    /// against the GC, and nobody can ever see it. Capability outputs used to
    /// be inserted exactly like that.
    #[test]
    fn user_visible_blob_without_owner_is_unreachable() {
        let db = crate::open_in_memory().unwrap();
        upsert(&db, &row("sha256:orphan", None, None)).unwrap();
        upsert(
            &db,
            &row("sha256:owned", Some("client-a"), Some("translate/x.txt")),
        )
        .unwrap();

        let listed = list_user_visible(&db, Some(1), Some("client-a"), 50).unwrap();
        let hashes: Vec<_> = listed.iter().map(|r| r.hash.as_str()).collect();
        assert_eq!(hashes, vec!["sha256:owned"]);
        assert_eq!(listed[0].path.as_deref(), Some("translate/x.txt"));
    }

    #[test]
    fn path_survives_round_trip_and_first_one_wins() {
        let db = crate::open_in_memory().unwrap();
        upsert(&db, &row("sha256:x", Some("c"), Some("first.txt"))).unwrap();
        upsert(&db, &row("sha256:x", Some("c"), Some("second.txt"))).unwrap();
        let rec = get(&db, "sha256:x").unwrap().unwrap();
        assert_eq!(rec.path.as_deref(), Some("first.txt"));
    }
}
