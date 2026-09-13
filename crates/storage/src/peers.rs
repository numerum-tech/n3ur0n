use serde::{Deserialize, Serialize};

use crate::{Db, StorageResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    pub id: String,
    pub endpoint: String,
    pub alias: Option<String>,
    pub last_seen: Option<i64>,
    pub tls_fingerprint: Option<String>,
    pub describe_self_cached: Option<String>,
    pub describe_self_fetched_at: Option<i64>,
    pub source: Option<String>,
}

/// Insert or update a peer.
///
/// `alias` is COALESCEd on purpose: a `None` from this path means *this writer
/// does not know*, not *the peer has no alias*. The reverse-announce path
/// learns a caller's endpoint from a signed envelope and nothing else, and an
/// unconditional write erased the alias a real `describe_self` had just
/// stored — every peer in the cluster lost its name after one exchange.
/// [`set_alias`] is how a writer that does know says so.
pub fn upsert(db: &Db, record: &PeerRecord) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO peers(id, endpoint, alias, last_seen, tls_fingerprint,
                           describe_self_cached, describe_self_fetched_at, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
             endpoint = excluded.endpoint,
             alias = COALESCE(excluded.alias, peers.alias),
             last_seen = COALESCE(excluded.last_seen, peers.last_seen),
             tls_fingerprint = COALESCE(excluded.tls_fingerprint, peers.tls_fingerprint),
             describe_self_cached = COALESCE(excluded.describe_self_cached, peers.describe_self_cached),
             describe_self_fetched_at = COALESCE(excluded.describe_self_fetched_at, peers.describe_self_fetched_at),
             source = COALESCE(excluded.source, peers.source)",
        rusqlite::params![
            record.id,
            record.endpoint,
            record.alias,
            record.last_seen,
            record.tls_fingerprint,
            record.describe_self_cached,
            record.describe_self_fetched_at,
            record.source,
        ],
    )?;
    Ok(())
}

/// Record what a peer calls itself, as learned from its `describe_self`.
///
/// Separate from [`upsert`] because only a descriptor is authoritative: it can
/// say "no alias" and mean it, which is why this takes an `Option` and writes
/// it as given.
pub fn set_alias(db: &Db, id: &str, alias: Option<&str>) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "UPDATE peers SET alias = ?2 WHERE id = ?1",
        rusqlite::params![id, alias],
    )?;
    Ok(())
}

/// Peers whose canonical id starts with `prefix`.
///
/// `GLOB` with a literal prefix is the form SQLite resolves through the `id`
/// primary-key index, so this is a range scan rather than a read of the whole
/// directory. Returns every match, because two peers sharing a prefix is an
/// ambiguity the caller has to see rather than a tie to break arbitrarily.
///
/// `prefix` is a fragment of an **id**, never a name: a peer serves its alias
/// bare (`toolbox`), and it is this node that composes `toolbox#ynhr3l57` for
/// display, so by the time a mention gets here the readable half has already
/// been discarded. The fragment is rejected unless it is plain lowercase
/// base32 — the shape an id has — because it ends up inside a `GLOB` pattern:
/// without the check, `@peer:*` would expand to every peer in the directory
/// and silently widen a scope meant to narrow one.
pub fn find_by_id_prefix(db: &Db, prefix: &str, limit: i64) -> StorageResult<Vec<PeerRecord>> {
    let body = prefix.strip_prefix("n3:").unwrap_or(prefix);
    if body.is_empty()
        || !body
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Ok(Vec::new());
    }
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, endpoint, alias, last_seen, tls_fingerprint,
                describe_self_cached, describe_self_fetched_at, source
         FROM peers WHERE id GLOB ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![format!("n3:{body}*"), limit], |row| {
        Ok(PeerRecord {
            id: row.get(0)?,
            endpoint: row.get(1)?,
            alias: row.get(2)?,
            last_seen: row.get(3)?,
            tls_fingerprint: row.get(4)?,
            describe_self_cached: row.get(5)?,
            describe_self_fetched_at: row.get(6)?,
            source: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get(db: &Db, id: &str) -> StorageResult<Option<PeerRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, endpoint, alias, last_seen, tls_fingerprint,
                describe_self_cached, describe_self_fetched_at, source
         FROM peers WHERE id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(PeerRecord {
            id: row.get(0)?,
            endpoint: row.get(1)?,
            alias: row.get(2)?,
            last_seen: row.get(3)?,
            tls_fingerprint: row.get(4)?,
            describe_self_cached: row.get(5)?,
            describe_self_fetched_at: row.get(6)?,
            source: row.get(7)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn list(db: &Db, limit: i64) -> StorageResult<Vec<PeerRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, endpoint, alias, last_seen, tls_fingerprint,
                describe_self_cached, describe_self_fetched_at, source
         FROM peers ORDER BY last_seen DESC NULLS LAST LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(PeerRecord {
            id: row.get(0)?,
            endpoint: row.get(1)?,
            alias: row.get(2)?,
            last_seen: row.get(3)?,
            tls_fingerprint: row.get(4)?,
            describe_self_cached: row.get(5)?,
            describe_self_fetched_at: row.get(6)?,
            source: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete a peer by instance id. Returns `true` if a row was removed.
pub fn delete(db: &Db, id: &str) -> StorageResult<bool> {
    let conn = db.get()?;
    let n = conn.execute("DELETE FROM peers WHERE id = ?1", [id])?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    #[test]
    fn an_alias_survives_a_writer_that_does_not_know_it() {
        let db = crate::open_in_memory().unwrap();
        let mut p = PeerRecord {
            id: "n3:peer".into(),
            endpoint: "https://x.example".into(),
            alias: Some("toolbox".into()),
            last_seen: Some(1),
            tls_fingerprint: None,
            describe_self_cached: None,
            describe_self_fetched_at: None,
            source: Some("manual".into()),
        };
        upsert(&db, &p).unwrap();

        // Reverse-announce: a signed call arrives, we learn the endpoint and
        // nothing else. The name must not evaporate.
        p.alias = None;
        p.endpoint = "http://new-host:4242".into();
        upsert(&db, &p).unwrap();
        let got = get(&db, "n3:peer").unwrap().unwrap();
        assert_eq!(got.alias.as_deref(), Some("toolbox"));
        assert_eq!(got.endpoint, "http://new-host:4242");

        // A descriptor is authoritative, including when it says "no alias".
        set_alias(&db, "n3:peer", None).unwrap();
        assert!(get(&db, "n3:peer").unwrap().unwrap().alias.is_none());
        set_alias(&db, "n3:peer", Some("renamed")).unwrap();
        assert_eq!(
            get(&db, "n3:peer").unwrap().unwrap().alias.as_deref(),
            Some("renamed")
        );
    }

    #[test]
    fn find_by_id_prefix_matches_on_the_index_and_surfaces_ambiguity() {
        let db = crate::open_in_memory().unwrap();
        for id in ["n3:aaaa1111", "n3:aaaa2222", "n3:bbbb3333"] {
            upsert(
                &db,
                &PeerRecord {
                    id: id.into(),
                    endpoint: format!("https://{id}.example"),
                    alias: None,
                    last_seen: Some(1),
                    tls_fingerprint: None,
                    describe_self_cached: None,
                    describe_self_fetched_at: None,
                    source: None,
                },
            )
            .unwrap();
        }
        assert_eq!(find_by_id_prefix(&db, "bbbb", 10).unwrap().len(), 1);
        // A prefix two peers share is an ambiguity, not a tie to break.
        assert_eq!(find_by_id_prefix(&db, "aaaa", 10).unwrap().len(), 2);
        assert_eq!(find_by_id_prefix(&db, "aaaa1", 10).unwrap().len(), 1);
        // The `n3:` prefix is accepted and stripped.
        assert_eq!(find_by_id_prefix(&db, "n3:bbbb", 10).unwrap().len(), 1);
        // Anything that is not an id shape matches nothing — no GLOB wildcard
        // ever reaches the pattern.
        assert!(find_by_id_prefix(&db, "*", 10).unwrap().is_empty());
        assert!(find_by_id_prefix(&db, "AAAA", 10).unwrap().is_empty());
        assert!(find_by_id_prefix(&db, "", 10).unwrap().is_empty());
    }

    #[test]
    fn upsert_and_get() {
        let db = open_in_memory().unwrap();
        let rec = PeerRecord {
            id: "n3:abc".into(),
            endpoint: "https://x.example".into(),
            alias: Some("@x".into()),
            last_seen: Some(1),
            tls_fingerprint: None,
            describe_self_cached: None,
            describe_self_fetched_at: None,
            source: Some("manual".into()),
        };
        upsert(&db, &rec).unwrap();
        let got = get(&db, "n3:abc").unwrap().unwrap();
        assert_eq!(got.endpoint, rec.endpoint);
        assert!(delete(&db, "n3:abc").unwrap());
        assert!(get(&db, "n3:abc").unwrap().is_none());
        assert!(!delete(&db, "n3:abc").unwrap());
    }
}
