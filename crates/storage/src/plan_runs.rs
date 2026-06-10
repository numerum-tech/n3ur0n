//! Plan runs repo — durability journal for planner dispatches (phase 1).
//!
//! One row per compiled plan, keyed by an opaque `id`. The node inserts a
//! row with `status = 'running'` before executing a plan and closes it to
//! `'done'` / `'failed'` afterwards. A `'running'` row whose process crashed
//! mid-execution stays detectable as an orphan; automatic resume is out of
//! scope here.

use crate::{Db, StorageResult};

#[derive(Debug, Clone)]
pub struct PlanRunRecord {
    pub id: String,
    pub conversation_id: String,
    pub plan_json: String,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

/// Insert a new plan run (typically `status = "running"`, `finished_at = None`).
pub fn insert(db: &Db, run: &PlanRunRecord) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO plan_runs(id, conversation_id, plan_json, status, created_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            run.id,
            run.conversation_id,
            run.plan_json,
            run.status,
            run.created_at,
            run.finished_at,
        ],
    )?;
    Ok(())
}

/// Close a run: set its `status` and `finished_at`.
pub fn set_status(db: &Db, id: &str, status: &str, finished_at: i64) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "UPDATE plan_runs SET status = ?1, finished_at = ?2 WHERE id = ?3",
        rusqlite::params![status, finished_at, id],
    )?;
    Ok(())
}

/// Fetch a single run by id.
pub fn get(db: &Db, id: &str) -> StorageResult<Option<PlanRunRecord>> {
    let conn = db.get()?;
    let row = conn
        .query_row(
            "SELECT id, conversation_id, plan_json, status, created_at, finished_at
             FROM plan_runs WHERE id = ?1",
            [id],
            |row| {
                Ok(PlanRunRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    plan_json: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    finished_at: row.get(5)?,
                })
            },
        )
        .ok();
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    fn seed_conversation(db: &Db, id: &str) {
        use crate::conversations::{self, ConversationRecord};
        conversations::insert(
            db,
            &ConversationRecord {
                id: id.into(),
                client_id: "client".into(),
                title: None,
                created_at: 0,
                updated_at: 0,
            },
        )
        .unwrap();
    }

    #[test]
    fn insert_then_close() {
        let db = open_in_memory().unwrap();
        seed_conversation(&db, "conv1");
        insert(
            &db,
            &PlanRunRecord {
                id: "run1".into(),
                conversation_id: "conv1".into(),
                plan_json: "{\"plan\":[]}".into(),
                status: "running".into(),
                created_at: 100,
                finished_at: None,
            },
        )
        .unwrap();

        let r = get(&db, "run1").unwrap().unwrap();
        assert_eq!(r.status, "running");
        assert_eq!(r.finished_at, None);

        set_status(&db, "run1", "done", 200).unwrap();
        let r = get(&db, "run1").unwrap().unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(r.finished_at, Some(200));
    }
}
