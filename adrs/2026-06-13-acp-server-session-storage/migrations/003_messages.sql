CREATE TABLE IF NOT EXISTS messages (
    id             TEXT PRIMARY KEY,
    prompt_turn_id TEXT NOT NULL REFERENCES prompt_turns(id),
    role           TEXT NOT NULL,
    content        TEXT NOT NULL DEFAULT '',
    position       INTEGER NOT NULL,
    created_at     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_turn
    ON messages(prompt_turn_id, position);
CREATE INDEX IF NOT EXISTS idx_sessions_fork_point
    ON sessions(fork_point_turn_id);
CREATE INDEX IF NOT EXISTS idx_sessions_updated
    ON sessions(updated_at DESC);
