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
