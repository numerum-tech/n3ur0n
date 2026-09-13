-- Third direction in the call journal: `local`.
--
-- `in` and `out` describe the network — a peer called us, or we called a peer.
-- Neither covers the commonest case on a node that serves its own user: the
-- planner running one of *this* instance's capabilities. Those executions were
-- invisible, so the Activity view's "most solicited capabilities" ranked only
-- the meta verbs a bootstrap crawl happens to send.
--
-- SQLite cannot widen a CHECK constraint in place, so the table is rebuilt.
-- Existing rows carry over: the journal is data an operator may already be
-- looking at.

ALTER TABLE audit_log RENAME TO audit_log_old;

CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp  INTEGER NOT NULL,
    direction  TEXT NOT NULL CHECK (direction IN ('in', 'out', 'local')),
    peer_id    TEXT NOT NULL,
    capability TEXT,
    status     TEXT NOT NULL,
    latency_ms INTEGER
);

INSERT INTO audit_log(id, timestamp, direction, peer_id, capability, status, latency_ms)
SELECT id, timestamp, direction, peer_id, capability, status, latency_ms FROM audit_log_old;

DROP TABLE audit_log_old;

CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_peer ON audit_log(peer_id);
-- The dashboard always filters on direction within a time window; without
-- this the aggregates scan every row the retention keeps.
CREATE INDEX idx_audit_direction_time ON audit_log(direction, timestamp);
