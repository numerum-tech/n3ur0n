-- Blob index (publisher + consumer mirror for classes A/B/D).
-- See n3ur0n-blob-protocol-v0.md §5.4.

CREATE TABLE IF NOT EXISTS blobs (
    hash TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mime TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    storage_path TEXT NOT NULL,

    provenance TEXT NOT NULL,
    role TEXT NOT NULL,
    anchor_kind TEXT NOT NULL,
    processing_status TEXT NOT NULL DEFAULT 'staged',

    local_user_id INTEGER,
    client_id TEXT,
    conversation_id TEXT,
    dispatch_id TEXT,

    capability TEXT,
    remote_sender_id TEXT,
    ticket_nonce TEXT,
    invoke_id TEXT,

    user_visible INTEGER NOT NULL DEFAULT 0,
    user_deletable INTEGER NOT NULL DEFAULT 0,

    uploader_id TEXT,
    recipients_whitelist TEXT,

    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    last_access_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_blobs_user_visible
    ON blobs(user_visible, local_user_id, client_id);
CREATE INDEX IF NOT EXISTS idx_blobs_cap_job
    ON blobs(anchor_kind, capability);
CREATE INDEX IF NOT EXISTS idx_blobs_expires
    ON blobs(expires_at);
CREATE INDEX IF NOT EXISTS idx_blobs_uploader
    ON blobs(uploader_id);
