//! n3ur0n-storage
//!
//! SQLite-backed repos. Schema covers: peers, nonces (anti-replay),
//! subscriptions, capabilities, audit_log. See architecture spec §6.

use std::path::Path;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use thiserror::Error;

pub type Db = Pool<SqliteConnectionManager>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("r2d2 pool: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration: {0}")]
    Migration(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_conversations.sql"),
];

pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Db> {
    let manager = SqliteConnectionManager::file(path).with_init(|conn| {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Ok(())
    });
    // r2d2 default min_idle/max_size of 8 races during first-time WAL setup.
    // Build a single-connection pool first so migrations run cleanly, then
    // resize.
    let pool = Pool::builder().max_size(8).min_idle(Some(1)).build(manager)?;
    migrate(&pool)?;
    Ok(pool)
}

pub fn open_in_memory() -> StorageResult<Db> {
    let manager = SqliteConnectionManager::memory();
    let pool = Pool::builder().max_size(1).build(manager)?;
    migrate(&pool)?;
    Ok(pool)
}

fn migrate(pool: &Db) -> StorageResult<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);",
    )?;
    let current: i64 = tx
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))?;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let v = (idx + 1) as i64;
        if v > current {
            tx.execute_batch(sql)
                .map_err(|e| StorageError::Migration(format!("v{v}: {e}")))?;
            tx.execute("INSERT INTO schema_version(version) VALUES (?1)", [v])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub mod conversations;
pub mod peers;
pub mod nonces;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_runs_migrations() {
        let pool = open_in_memory().unwrap();
        let conn = pool.get().unwrap();
        let v: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }
}
