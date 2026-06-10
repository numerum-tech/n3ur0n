-- plan_runs: durability journal for planner dispatches (phase 1, write-only).
-- One row per compiled plan. Inserted with status='running' before execution,
-- closed to 'done' after reflect or 'failed' on error. A 'running' row whose
-- process died is an orphan, detectable by status; automatic resume is out of
-- scope for phase 1.
CREATE TABLE plan_runs (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    plan_json       TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed')),
    created_at      INTEGER NOT NULL,    -- unix epoch seconds
    finished_at     INTEGER,             -- NULL while running
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX idx_plan_runs_conv ON plan_runs(conversation_id);
