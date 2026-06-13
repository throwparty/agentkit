CREATE TABLE IF NOT EXISTS prompt_turns (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_id  TEXT REFERENCES prompt_turns(id),
    position   INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_prompt_turns_session
    ON prompt_turns(session_id, position);
CREATE INDEX IF NOT EXISTS idx_prompt_turns_parent
    ON prompt_turns(parent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_head
    ON sessions(head_prompt_turn_id);
CREATE INDEX IF NOT EXISTS idx_sessions_forked_from
    ON sessions(forked_from_session_id);
