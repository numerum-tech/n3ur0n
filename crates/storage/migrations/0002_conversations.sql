-- Conversations: first-class persistent threads, isolated per browser/Tauri
-- client via the `client_id` column. See planner brainstorm doc.

CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    client_id   TEXT NOT NULL,
    title       TEXT,
    created_at  INTEGER NOT NULL,    -- unix epoch seconds
    updated_at  INTEGER NOT NULL
);

CREATE INDEX idx_conversations_client_updated
    ON conversations(client_id, updated_at DESC);

-- conversation_turns: append-only ledger of turns per conversation.
-- `seq` strictly increasing per conversation_id; `payload` is JSON-encoded
-- Turn enum (User | Assistant | ToolCall | ToolResult | System).
CREATE TABLE conversation_turns (
    conversation_id TEXT NOT NULL,
    seq             INTEGER NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'tool_call', 'tool_result', 'system')),
    payload         TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, seq),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX idx_turns_created ON conversation_turns(created_at);
