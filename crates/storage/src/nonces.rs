use crate::{Db, StorageResult};

/// Insert a nonce. Returns `Ok(true)` if newly inserted, `Ok(false)` if already seen (replay).
pub fn insert_if_absent(
    db: &Db,
    sender_id: &str,
    nonce: &str,
    seen_at: i64,
) -> StorageResult<bool> {
    let conn = db.get()?;
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO nonces(sender_id, nonce, seen_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![sender_id, nonce, seen_at],
    )?;
    Ok(inserted == 1)
}

/// Drop nonces older than `cutoff` (unix seconds). Returns rows deleted.
pub fn prune_older_than(db: &Db, cutoff: i64) -> StorageResult<usize> {
    let conn = db.get()?;
    let n = conn.execute("DELETE FROM nonces WHERE seen_at < ?1", [cutoff])?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    #[test]
    fn replay_detected() {
        let db = open_in_memory().unwrap();
        assert!(insert_if_absent(&db, "n3:a", "x", 100).unwrap());
        assert!(!insert_if_absent(&db, "n3:a", "x", 100).unwrap());
    }

    #[test]
    fn prune_drops_old() {
        let db = open_in_memory().unwrap();
        insert_if_absent(&db, "n3:a", "old", 1).unwrap();
        insert_if_absent(&db, "n3:a", "new", 1000).unwrap();
        let n = prune_older_than(&db, 500).unwrap();
        assert_eq!(n, 1);
    }
}
