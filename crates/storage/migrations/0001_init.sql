-- Initial N3UR0N schema. See project-tech-stack.md §6.2.

CREATE TABLE peers (
    id                       TEXT PRIMARY KEY,           -- canonical instance id
    endpoint                 TEXT NOT NULL,
    alias                    TEXT,
    last_seen                INTEGER,                    -- unix epoch seconds
    tls_fingerprint          TEXT,
    describe_self_cached     TEXT,                       -- JSON blob
    describe_self_fetched_at INTEGER,
    source                   TEXT
);

CREATE INDEX idx_peers_last_seen ON peers(last_seen);

CREATE TABLE nonces (
    sender_id TEXT NOT NULL,
    nonce     TEXT NOT NULL,
    seen_at   INTEGER NOT NULL,
    PRIMARY KEY (sender_id, nonce)
);

CREATE INDEX idx_nonces_seen_at ON nonces(seen_at);

CREATE TABLE subscriptions (
    peer_id         TEXT NOT NULL,
    capability_name TEXT NOT NULL,
    token           TEXT NOT NULL,
    granted_at      INTEGER NOT NULL,
    expires_at      INTEGER,
    PRIMARY KEY (peer_id, capability_name)
);

CREATE TABLE capabilities (
    name        TEXT PRIMARY KEY,
    mode        TEXT NOT NULL CHECK (mode IN ('free', 'restricted')),
    schema_in   TEXT NOT NULL,
    schema_out  TEXT NOT NULL,
    description TEXT NOT NULL,
    tags        TEXT NOT NULL DEFAULT '[]',
    lobe_ids    TEXT NOT NULL DEFAULT '[]',
    backend_ref TEXT
);

CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp  INTEGER NOT NULL,
    direction  TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    peer_id    TEXT NOT NULL,
    capability TEXT,
    status     TEXT NOT NULL,
    latency_ms INTEGER
);

CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_peer ON audit_log(peer_id);
