CREATE TABLE IF NOT EXISTS sessions (
    id                    TEXT PRIMARY KEY,
    head_prompt_turn_id   TEXT,
    forked_from_session_id TEXT,
    fork_point_turn_id    TEXT,
    cwd                   TEXT NOT NULL DEFAULT '',
    title                 TEXT NOT NULL DEFAULT '',
    mode                  TEXT,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    active                INTEGER NOT NULL DEFAULT 1,
    transport             TEXT NOT NULL DEFAULT ''
);
