//! Conversations + turns repo.
//!
//! Conversations are persistent threads owned by a client (browser / Tauri
//! installation) identified by `client_id`. Turns form an append-only ledger
//! keyed by `(conversation_id, seq)` where `seq` is monotonically increasing.

use crate::{Db, StorageResult};

#[derive(Debug, Clone)]
pub struct ConversationRecord {
    pub id: String,
    pub client_id: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct TurnRecord {
    pub conversation_id: String,
    pub seq: i64,
    pub role: String,
    /// JSON-encoded payload (Turn enum on the node side).
    pub payload: String,
    pub created_at: i64,
}

/// Insert a new conversation.
pub fn insert(db: &Db, conv: &ConversationRecord) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO conversations(id, client_id, title, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            conv.id,
            conv.client_id,
            conv.title,
            conv.created_at,
            conv.updated_at,
        ],
    )?;
    Ok(())
}

/// Look up a conversation. Caller is responsible for verifying ownership
/// against `client_id` before exposing data.
pub fn get(db: &Db, id: &str) -> StorageResult<Option<ConversationRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, client_id, title, created_at, updated_at
         FROM conversations WHERE id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(ConversationRecord {
            id: row.get(0)?,
            client_id: row.get(1)?,
            title: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

/// List a client's conversations (most recently updated first).
pub fn list_for_client(
    db: &Db,
    client_id: &str,
    limit: i64,
) -> StorageResult<Vec<ConversationRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT id, client_id, title, created_at, updated_at
         FROM conversations WHERE client_id = ?1
         ORDER BY updated_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![client_id, limit], |row| {
        Ok(ConversationRecord {
            id: row.get(0)?,
            client_id: row.get(1)?,
            title: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Update title and/or updated_at.
pub fn update_meta(
    db: &Db,
    id: &str,
    title: Option<&str>,
    updated_at: i64,
) -> StorageResult<()> {
    let conn = db.get()?;
    if let Some(title) = title {
        conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![title, updated_at, id],
        )?;
    } else {
        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![updated_at, id],
        )?;
    }
    Ok(())
}

/// Delete a conversation and (via FK cascade) all its turns.
pub fn delete(db: &Db, id: &str) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
    Ok(())
}

/// Append a turn within a transaction that also bumps `updated_at`.
pub fn append_turn(db: &Db, turn: &TurnRecord, conv_updated_at: i64) -> StorageResult<()> {
    let mut conn = db.get()?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO conversation_turns(conversation_id, seq, role, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            turn.conversation_id,
            turn.seq,
            turn.role,
            turn.payload,
            turn.created_at,
        ],
    )?;
    tx.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![conv_updated_at, turn.conversation_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Load all turns for a conversation, ordered by `seq` ascending.
pub fn load_turns(db: &Db, conversation_id: &str) -> StorageResult<Vec<TurnRecord>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT conversation_id, seq, role, payload, created_at
         FROM conversation_turns WHERE conversation_id = ?1
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([conversation_id], |row| {
        Ok(TurnRecord {
            conversation_id: row.get(0)?,
            seq: row.get(1)?,
            role: row.get(2)?,
            payload: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Highest seq currently stored, or `None` if no turns.
pub fn max_seq(db: &Db, conversation_id: &str) -> StorageResult<Option<i64>> {
    let conn = db.get()?;
    let v: Option<i64> = conn
        .query_row(
            "SELECT MAX(seq) FROM conversation_turns WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        )
        .ok();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    fn ts() -> i64 {
        1_700_000_000
    }

    #[test]
    fn insert_get_list() {
        let db = open_in_memory().unwrap();
        let conv = ConversationRecord {
            id: "c1".into(),
            client_id: "alice".into(),
            title: Some("hello".into()),
            created_at: ts(),
            updated_at: ts(),
        };
        insert(&db, &conv).unwrap();
        let got = get(&db, "c1").unwrap().unwrap();
        assert_eq!(got.client_id, "alice");

        let conv2 = ConversationRecord {
            id: "c2".into(),
            client_id: "alice".into(),
            title: None,
            created_at: ts() + 1,
            updated_at: ts() + 1,
        };
        insert(&db, &conv2).unwrap();
        let conv3 = ConversationRecord {
            id: "c3".into(),
            client_id: "bob".into(),
            title: None,
            created_at: ts() + 2,
            updated_at: ts() + 2,
        };
        insert(&db, &conv3).unwrap();

        let alice = list_for_client(&db, "alice", 10).unwrap();
        assert_eq!(alice.len(), 2);
        assert_eq!(alice[0].id, "c2"); // most recent first
        let bob = list_for_client(&db, "bob", 10).unwrap();
        assert_eq!(bob.len(), 1);
    }

    #[test]
    fn append_and_load_turns() {
        let db = open_in_memory().unwrap();
        insert(
            &db,
            &ConversationRecord {
                id: "c1".into(),
                client_id: "alice".into(),
                title: None,
                created_at: ts(),
                updated_at: ts(),
            },
        )
        .unwrap();

        for seq in 1..=3 {
            append_turn(
                &db,
                &TurnRecord {
                    conversation_id: "c1".into(),
                    seq,
                    role: "user".into(),
                    payload: format!("{{\"content\":\"msg{}\"}}", seq),
                    created_at: ts() + seq,
                },
                ts() + seq,
            )
            .unwrap();
        }

        let turns = load_turns(&db, "c1").unwrap();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].seq, 1);
        assert_eq!(turns[2].seq, 3);

        let max = max_seq(&db, "c1").unwrap();
        assert_eq!(max, Some(3));
    }

    #[test]
    fn delete_cascades_turns() {
        let db = open_in_memory().unwrap();
        insert(
            &db,
            &ConversationRecord {
                id: "c1".into(),
                client_id: "alice".into(),
                title: None,
                created_at: ts(),
                updated_at: ts(),
            },
        )
        .unwrap();
        append_turn(
            &db,
            &TurnRecord {
                conversation_id: "c1".into(),
                seq: 1,
                role: "user".into(),
                payload: "{}".into(),
                created_at: ts(),
            },
            ts(),
        )
        .unwrap();

        delete(&db, "c1").unwrap();
        let turns = load_turns(&db, "c1").unwrap();
        assert!(turns.is_empty());
        assert!(get(&db, "c1").unwrap().is_none());
    }
}
