-- Migration 0001: initial schema (adopted from schema.sql / the storage ADR).
-- IDs are bare UUIDs; sess_/msg_ prefixes are added at serialization boundaries.

CREATE TABLE sessions (
    id                     TEXT PRIMARY KEY,
    head_turn_id           TEXT REFERENCES turns(id),
    kind                   TEXT NOT NULL DEFAULT 'interactive', -- interactive | ephemeral
    forked_from_session_id TEXT REFERENCES sessions(id),
    fork_point_turn_id     TEXT REFERENCES turns(id),
    cwd                    TEXT NOT NULL DEFAULT '',
    title                  TEXT NOT NULL DEFAULT '',
    owner_connection       TEXT,
    lease_expires_at       INTEGER,
    active                 INTEGER NOT NULL DEFAULT 1, -- 0 = soft-deleted; close does NOT deactivate
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    metadata               TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE turns (
    id                     TEXT PRIMARY KEY,
    session_id             TEXT NOT NULL REFERENCES sessions(id),
    parent_id              TEXT REFERENCES turns(id),
    kind                   TEXT NOT NULL DEFAULT 'interaction', -- interaction | seed | compaction
    first_retained_turn_id TEXT REFERENCES turns(id),
    created_at             INTEGER NOT NULL,
    input_tokens           INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    cost_usd               REAL NOT NULL DEFAULT 0,
    metadata               TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE messages (
    id                     TEXT PRIMARY KEY,
    turn_id                TEXT NOT NULL REFERENCES turns(id),
    role                   TEXT NOT NULL, -- user | assistant | tool_call | tool_result | system
    content                TEXT NOT NULL DEFAULT '[]', -- JSON array of ACP content blocks
    tool_name              TEXT,
    tool_call_id           TEXT REFERENCES messages(id),
    is_error               INTEGER,
    position               INTEGER NOT NULL,
    created_at             INTEGER NOT NULL,
    metadata               TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE definitions (
    hash          TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    name          TEXT NOT NULL,
    version       TEXT,
    content       TEXT NOT NULL,
    recorded_at   INTEGER NOT NULL
);

CREATE TABLE turn_definitions (
    turn_id         TEXT NOT NULL REFERENCES turns(id),
    definition_hash TEXT NOT NULL REFERENCES definitions(hash),
    PRIMARY KEY (turn_id, definition_hash)
);

CREATE INDEX idx_turns_parent         ON turns(parent_id);
CREATE INDEX idx_turns_session        ON turns(session_id, created_at);
CREATE INDEX idx_messages_turn        ON messages(turn_id, position);
CREATE INDEX idx_sessions_forked_from ON sessions(forked_from_session_id);
CREATE INDEX idx_sessions_fork_point  ON sessions(fork_point_turn_id);
CREATE INDEX idx_sessions_updated     ON sessions(updated_at DESC);
CREATE INDEX idx_turn_definitions_hash ON turn_definitions(definition_hash);
