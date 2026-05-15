-- RBAC: users + sessions.
--
-- Roles are stored as TEXT for forward-compat; the auth layer maps them
-- to a const permission set. Adding a role = code change, not schema.
-- See `crates/node/src/auth.rs` (or wherever the role map lands).
--
-- `password_hash` is argon2id PHC string (full self-describing format,
-- includes algorithm + params + salt). No separate salt column.
--
-- `sessions.token_hash` stores SHA-256 of the opaque cookie value, not
-- the cookie itself — DB compromise can't impersonate live sessions.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('user', 'operator', 'admin')),
    created_at    INTEGER NOT NULL,
    last_login    INTEGER
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);
