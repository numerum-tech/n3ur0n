//! Call journal: one row per signed exchange this instance took part in.
//!
//! The table has existed since the first migration and nothing ever wrote to
//! it. It backs the Activity view, whose question is not "what happened at
//! 14:03" but "what is this node doing right now, and what does it
//! contribute": `direction` separates the calls the node **serves** (`in`)
//! from the calls it **makes** (`out`), which is the whole distinction between
//! contributing to the network and consuming it.
//!
//! Writes are best-effort by design — a node must answer a peer even when it
//! cannot journal the fact — so every caller logs a failure and carries on.

use serde::{Deserialize, Serialize};

use crate::{Db, StorageResult};

/// Which side of the exchange this instance was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// A peer called us: this is what the node contributes.
    In,
    /// We called a peer: this is what the node consumes.
    Out,
    /// We ran one of our own capabilities for our own user. Neither
    /// contribution nor consumption, and the commonest case on a node that
    /// serves somebody: leaving it out made "most solicited capabilities"
    /// rank only the meta verbs a bootstrap crawl sends.
    Local,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
            Direction::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: i64,
    pub direction: Direction,
    /// The other end: the caller for `in`, the callee for `out`.
    pub peer_id: String,
    /// The capability invoked, or the meta verb (`ping`, `describe_self`,
    /// `get_known_peers`) — a node's external surface is more than `invoke`.
    pub capability: Option<String>,
    /// `ok` or a short error kind. Never the error message: this table is
    /// read back into a dashboard, not into a debugger.
    pub status: String,
    pub latency_ms: Option<i64>,
}

pub fn record(db: &Db, entry: &AuditEntry) -> StorageResult<()> {
    let conn = db.get()?;
    conn.execute(
        "INSERT INTO audit_log(timestamp, direction, peer_id, capability, status, latency_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            entry.timestamp,
            entry.direction.as_str(),
            entry.peer_id,
            entry.capability,
            entry.status,
            entry.latency_ms,
        ],
    )?;
    Ok(())
}

/// Totals for one direction since `since`, as `(calls, errors, p50 latency)`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectionStats {
    pub calls: i64,
    pub errors: i64,
    /// Median over the rows that carry a latency, `None` when none do.
    /// Median rather than mean: a single peer timing out at 30 s would drag
    /// a mean across the whole dashboard. On an even count this is the upper
    /// of the two middle values, which is close enough for a gauge.
    pub median_latency_ms: Option<i64>,
}

/// Inbound capability calls only — discovery excluded.
///
/// "Served" and "discovery" are shown side by side on the dashboard, so the
/// first counting the second made the two tiles double up and the label lie:
/// a node answering nothing but `describe_self` crawls read as having served
/// forty capability calls.
pub fn served_capability_stats(db: &Db, since: i64) -> StorageResult<DirectionStats> {
    let conn = db.get()?;
    const NOT_META: &str = "capability IS NULL OR capability NOT IN \
         ('describe_self', 'ping', 'get_known_peers', 'blob_ticket')";
    let (calls, errors): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(status <> 'ok'), 0)
             FROM audit_log WHERE direction = 'in' AND timestamp >= ?1 AND ({NOT_META})"
        ),
        [since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let median_latency_ms = if calls == 0 {
        None
    } else {
        conn.query_row(
            &format!(
                "SELECT latency_ms FROM audit_log
                 WHERE direction = 'in' AND timestamp >= ?1 AND latency_ms IS NOT NULL
                   AND ({NOT_META})
                 ORDER BY latency_ms
                 LIMIT 1 OFFSET (
                    SELECT COUNT(*) / 2 FROM audit_log
                    WHERE direction = 'in' AND timestamp >= ?1 AND latency_ms IS NOT NULL
                      AND ({NOT_META})
                 )"
            ),
            [since],
            |r| r.get::<_, i64>(0),
        )
        .ok()
    };
    Ok(DirectionStats {
        calls,
        errors,
        median_latency_ms,
    })
}

pub fn direction_stats(db: &Db, direction: Direction, since: i64) -> StorageResult<DirectionStats> {
    let conn = db.get()?;
    let (calls, errors): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(status <> 'ok'), 0)
         FROM audit_log WHERE direction = ?1 AND timestamp >= ?2",
        rusqlite::params![direction.as_str(), since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let median_latency_ms = if calls == 0 {
        None
    } else {
        conn.query_row(
            "SELECT latency_ms FROM audit_log
             WHERE direction = ?1 AND timestamp >= ?2 AND latency_ms IS NOT NULL
             ORDER BY latency_ms
             LIMIT 1 OFFSET (
                SELECT COUNT(*) / 2 FROM audit_log
                WHERE direction = ?1 AND timestamp >= ?2 AND latency_ms IS NOT NULL
             )",
            rusqlite::params![direction.as_str(), since],
            |r| r.get::<_, i64>(0),
        )
        .ok()
    };
    Ok(DirectionStats {
        calls,
        errors,
        median_latency_ms,
    })
}

/// One row of a "most solicited" ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ranked {
    pub key: String,
    pub calls: i64,
    pub errors: i64,
}

/// The meta verbs. They are journalled — a node's external surface really is
/// mostly discovery traffic — but they are not capabilities, and a bootstrap
/// crawl of four peers buries every real capability under them in a ranking.
pub const META_VERBS: [&str; 4] = ["describe_self", "ping", "get_known_peers", "blob_ticket"];

/// How many meta-verb exchanges this node answered: the discovery traffic.
pub fn meta_calls_since(db: &Db, since: i64) -> StorageResult<i64> {
    let conn = db.get()?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM audit_log
         WHERE direction = 'in' AND timestamp >= ?1
           AND capability IN ('describe_self', 'ping', 'get_known_peers', 'blob_ticket')",
        [since],
        |r| r.get(0),
    )?)
}

/// Capabilities ranked by how often they were *solicited*, whoever asked:
/// a peer invoking us (`in`) and our own planner running the same capability
/// (`local`) are both demand for it. Outbound calls are excluded — those are
/// somebody else's capability.
pub fn top_capabilities_solicited(db: &Db, since: i64, limit: i64) -> StorageResult<Vec<Ranked>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT capability, COUNT(*), COALESCE(SUM(status <> 'ok'), 0)
         FROM audit_log
         WHERE direction IN ('in', 'local') AND timestamp >= ?1 AND capability IS NOT NULL
           AND capability NOT IN ('describe_self', 'ping', 'get_known_peers', 'blob_ticket')
         GROUP BY capability ORDER BY COUNT(*) DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![since, limit], |row| {
        Ok(Ranked {
            key: row.get(0)?,
            calls: row.get(1)?,
            errors: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Capabilities ranked by call count, for one direction.
pub fn top_capabilities(
    db: &Db,
    direction: Direction,
    since: i64,
    limit: i64,
) -> StorageResult<Vec<Ranked>> {
    ranked(
        db,
        "SELECT capability, COUNT(*), COALESCE(SUM(status <> 'ok'), 0)
         FROM audit_log
         WHERE direction = ?1 AND timestamp >= ?2 AND capability IS NOT NULL
         GROUP BY capability ORDER BY COUNT(*) DESC LIMIT ?3",
        direction,
        since,
        limit,
    )
}

/// Peers ranked by call count, for one direction.
pub fn top_peers(
    db: &Db,
    direction: Direction,
    since: i64,
    limit: i64,
) -> StorageResult<Vec<Ranked>> {
    ranked(
        db,
        "SELECT peer_id, COUNT(*), COALESCE(SUM(status <> 'ok'), 0)
         FROM audit_log
         WHERE direction = ?1 AND timestamp >= ?2
         GROUP BY peer_id ORDER BY COUNT(*) DESC LIMIT ?3",
        direction,
        since,
        limit,
    )
}

fn ranked(
    db: &Db,
    sql: &str,
    direction: Direction,
    since: i64,
    limit: i64,
) -> StorageResult<Vec<Ranked>> {
    let conn = db.get()?;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![direction.as_str(), since, limit], |row| {
        Ok(Ranked {
            key: row.get(0)?,
            calls: row.get(1)?,
            errors: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Calls per time bucket, oldest first — the dashboard's pulse.
///
/// Buckets are computed in SQL and returned dense: a window with no traffic
/// still yields its zero, because a gap in a live chart has to read as
/// "nothing happened", not as "no data".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bucket {
    pub start: i64,
    pub inbound: i64,
    pub outbound: i64,
    pub local: i64,
    pub errors: i64,
}

pub fn buckets(db: &Db, since: i64, until: i64, width_secs: i64) -> StorageResult<Vec<Bucket>> {
    assert!(width_secs > 0, "bucket width must be positive");
    let conn = db.get()?;
    let mut stmt = conn.prepare(
        "SELECT (timestamp / ?3) * ?3 AS bucket,
                COALESCE(SUM(direction = 'in'), 0),
                COALESCE(SUM(direction = 'out'), 0),
                COALESCE(SUM(direction = 'local'), 0),
                COALESCE(SUM(status <> 'ok'), 0)
         FROM audit_log
         WHERE timestamp >= ?1 AND timestamp < ?2
         GROUP BY bucket",
    )?;
    let rows = stmt.query_map(rusqlite::params![since, until, width_secs], |row| {
        Ok(Bucket {
            start: row.get(0)?,
            inbound: row.get(1)?,
            outbound: row.get(2)?,
            local: row.get(3)?,
            errors: row.get(4)?,
        })
    })?;
    let found: Vec<Bucket> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let first = (since / width_secs) * width_secs;
    let mut out = Vec::new();
    let mut start = first;
    while start < until {
        let hit = found.iter().find(|b| b.start == start);
        out.push(match hit {
            Some(b) => b.clone(),
            None => Bucket {
                start,
                inbound: 0,
                outbound: 0,
                local: 0,
                errors: 0,
            },
        });
        start += width_secs;
    }
    Ok(out)
}

/// Drop rows older than `cutoff`. Returns how many went.
pub fn prune_older_than(db: &Db, cutoff: i64) -> StorageResult<usize> {
    let conn = db.get()?;
    Ok(conn.execute("DELETE FROM audit_log WHERE timestamp < ?1", [cutoff])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_in_memory;

    fn entry(ts: i64, dir: Direction, peer: &str, cap: &str, status: &str, ms: i64) -> AuditEntry {
        AuditEntry {
            timestamp: ts,
            direction: dir,
            peer_id: peer.into(),
            capability: Some(cap.into()),
            status: status.into(),
            latency_ms: Some(ms),
        }
    }

    fn seeded() -> Db {
        let db = open_in_memory().unwrap();
        for e in [
            entry(99, Direction::In, "n3:a", "describe_self", "ok", 1),
            entry(100, Direction::In, "n3:a", "time", "ok", 5),
            entry(101, Direction::In, "n3:a", "time", "ok", 15),
            entry(102, Direction::In, "n3:b", "time", "error", 7),
            entry(103, Direction::In, "n3:b", "reverse", "ok", 9),
            entry(160, Direction::Out, "n3:c", "chat", "ok", 400),
            entry(161, Direction::Out, "n3:c", "chat", "timeout", 30000),
            // Our own planner running our own capability.
            entry(104, Direction::Local, "n3:self", "time", "ok", 3),
            entry(105, Direction::Local, "n3:self", "random_int", "ok", 2),
        ] {
            record(&db, &e).unwrap();
        }
        db
    }

    #[test]
    fn stats_separate_what_we_serve_from_what_we_call() {
        let db = seeded();
        let inbound = direction_stats(&db, Direction::In, 0).unwrap();
        assert_eq!((inbound.calls, inbound.errors), (5, 1));
        let outbound = direction_stats(&db, Direction::Out, 0).unwrap();
        assert_eq!((outbound.calls, outbound.errors), (2, 1));
        // The window is respected.
        assert_eq!(direction_stats(&db, Direction::In, 102).unwrap().calls, 2);
    }

    #[test]
    fn the_median_is_not_dragged_by_one_slow_call() {
        let db = seeded();
        // Inbound latencies are 1, 5, 15, 7, 9 -> sorted 1, 5, 7, 9, 15.
        let inbound = direction_stats(&db, Direction::In, 0).unwrap();
        assert_eq!(inbound.median_latency_ms, Some(7));

        // Add one pathological call. The mean would jump past 5000; the
        // reported figure must barely move.
        record(
            &db,
            &entry(106, Direction::In, "n3:b", "time", "timeout", 30_000),
        )
        .unwrap();
        let inbound = direction_stats(&db, Direction::In, 0).unwrap();
        assert_eq!(inbound.median_latency_ms, Some(9));
    }

    #[test]
    fn solicitation_counts_our_own_use_alongside_a_peer_s() {
        let db = seeded();
        let caps = top_capabilities_solicited(&db, 0, 10).unwrap();
        // `time`: 3 inbound + 1 local. Ranking only inbound would have put it
        // at 3 and hidden the node's own use of its own capability entirely.
        assert_eq!(caps[0].key, "time");
        assert_eq!(caps[0].calls, 4);
        let names: Vec<&str> = caps.iter().map(|c| c.key.as_str()).collect();
        assert!(names.contains(&"random_int"), "{names:?}");
        // `chat` is outbound: somebody else's capability, not ours.
        assert!(!names.contains(&"chat"), "{names:?}");
        // Discovery traffic is counted, but not as a capability: one bootstrap
        // crawl of four peers would otherwise outrank everything real.
        assert!(!names.contains(&"describe_self"), "{names:?}");
        assert_eq!(meta_calls_since(&db, 0).unwrap(), 1);
        // The two tiles must partition the inbound traffic, not overlap:
        // 5 inbound = 4 capability calls + 1 discovery exchange.
        assert_eq!(direction_stats(&db, Direction::In, 0).unwrap().calls, 5);
        assert_eq!(served_capability_stats(&db, 0).unwrap().calls, 4);
    }

    #[test]
    fn rankings_are_by_call_count_and_carry_their_errors() {
        let db = seeded();
        let caps = top_capabilities(&db, Direction::In, 0, 10).unwrap();
        assert_eq!(caps[0].key, "time");
        assert_eq!((caps[0].calls, caps[0].errors), (3, 1));
        assert_eq!(caps[1].key, "reverse");

        // Peer rankings count every exchange, discovery included: "who talks
        // to this node" is a different question from "which capability".
        let peers = top_peers(&db, Direction::In, 0, 10).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].calls, 3, "n3:a sent describe_self plus two time");
    }

    #[test]
    fn buckets_are_dense_so_a_quiet_window_reads_as_zero() {
        let db = seeded();
        let b = buckets(&db, 100, 200, 20).unwrap();
        assert_eq!(b.len(), 5, "100..200 by 20s");
        assert_eq!(b[0].start, 100);
        assert_eq!(b[0].inbound, 4, "the four inbound in 100..120");
        assert_eq!(b[0].local, 2, "local executions are their own series");
        // 120..160 saw nothing at all, and says so rather than being absent.
        assert_eq!((b[1].inbound, b[1].outbound), (0, 0));
        assert_eq!((b[2].inbound, b[2].outbound), (0, 0));
        assert_eq!(b[3].outbound, 2, "160 and 161 land together");
        assert_eq!(b[3].errors, 1);
    }

    #[test]
    fn pruning_drops_only_what_is_older_than_the_cutoff() {
        let db = seeded();
        // Five inbound plus the two local executions, all below the cutoff.
        assert_eq!(prune_older_than(&db, 160).unwrap(), 7);
        assert_eq!(direction_stats(&db, Direction::In, 0).unwrap().calls, 0);
        assert_eq!(direction_stats(&db, Direction::Out, 0).unwrap().calls, 2);
    }
}
