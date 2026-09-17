-- tackle session store — migration 001
-- Extends adrs/2026-06-13-acp-server-session-storage; IDs are bare UUIDs,
-- sess_/msg_ prefixes are added at serialization boundaries.

PRAGMA journal_mode=WAL;
PRAGMA busy_timeout=5000;

CREATE TABLE sessions (
    id                     TEXT PRIMARY KEY,                    -- UUIDv4
    head_turn_id           TEXT REFERENCES turns(id),           -- NULL before first turn
    kind                   TEXT NOT NULL DEFAULT 'interactive', -- interactive | ephemeral
    forked_from_session_id TEXT REFERENCES sessions(id),        -- NULL for roots
    fork_point_turn_id     TEXT REFERENCES turns(id),           -- NULL for roots
    cwd                    TEXT NOT NULL DEFAULT '',
    title                  TEXT NOT NULL DEFAULT '',
    owner_connection       TEXT,                                -- ownership lease; NULL = unowned
    lease_expires_at       INTEGER,                             -- unix seconds; heartbeat refreshes
    active                 INTEGER NOT NULL DEFAULT 1,          -- 0 = soft-deleted (close does NOT deactivate; closed sessions stay listable)
    created_at             INTEGER NOT NULL,                    -- unix seconds
    updated_at             INTEGER NOT NULL,
    metadata               TEXT NOT NULL DEFAULT '{}'
);

-- Turns are DAG NODES, not events: the structure of a turn is exactly
-- (id, parent_id, kind, first_retained_turn_id); its payload is its messages.
-- session_id, created_at, and metadata are node attribution (provenance),
-- never part of assembly: forks walk across session boundaries.
-- usage_* are the DELTA this turn's model requests incurred (input, output,
-- cost); a session's cumulative cost is the sum over its OWN turns — ancestor
-- turns' costs belong to their creating sessions, so forks never double-count.
-- Acyclicity holds by application discipline: the FK requires the parent to
-- pre-exist, and appends only ever reference existing turns.
CREATE TABLE turns (
    id                     TEXT PRIMARY KEY,                    -- UUIDv4
    session_id             TEXT NOT NULL REFERENCES sessions(id), -- provenance: the session that appended this turn (GC, ownership queries)
    parent_id              TEXT REFERENCES turns(id),           -- NULL for roots
    kind                   TEXT NOT NULL DEFAULT 'interaction', -- interaction | seed | compaction
    first_retained_turn_id TEXT REFERENCES turns(id),           -- compaction turns only: oldest turn kept verbatim
    created_at             INTEGER NOT NULL,                    -- unix seconds
    input_tokens           INTEGER NOT NULL DEFAULT 0,          -- usage delta: input tokens consumed by this turn's requests
    output_tokens          INTEGER NOT NULL DEFAULT 0,          -- usage delta: output tokens
    cost_usd               REAL NOT NULL DEFAULT 0,             -- usage delta: cost incurred
    metadata               TEXT NOT NULL DEFAULT '{}'           -- attribution: actor, agent instance
);

-- Tool calls are TWO messages: role tool_call (the model's request: namespaced
-- invokable in tool_name, arguments JSON in content) and role tool_result (the
-- response or denial, referencing its request via tool_call_id, is_error set on
-- denial or failure). Both are always stored: context replay requires the pair,
-- and providers reject unpaired tool requests. Parallel tool use is multiple
-- pairs, position-ordered within the turn. A tool_call left dangling by
-- cancellation gets a synthetic cancelled tool_result at assembly time.
CREATE TABLE messages (
    id                     TEXT PRIMARY KEY,                    -- UUIDv4
    turn_id                TEXT NOT NULL REFERENCES turns(id),
    role                   TEXT NOT NULL,                       -- user | assistant | tool_call | tool_result | system
    content                TEXT NOT NULL DEFAULT '[]',          -- JSON array of ACP content blocks (text, image, ...);
                                                                -- tool_call: arguments JSON; tool_result: result blocks
    tool_name              TEXT,                                -- tool_call only: namespaced invokable
    tool_call_id           TEXT REFERENCES messages(id),        -- tool_result only: the request this answers
    is_error               INTEGER,                             -- tool_result only: denial or execution failure
    position               INTEGER NOT NULL,
    created_at             INTEGER NOT NULL,
    metadata               TEXT NOT NULL DEFAULT '{}'
);

-- Permission grants are IN-MEMORY ONLY: session-scoped decisions live for the
-- session's active life in the current process — gone on close, on process
-- exit, and on resume after restart. They are deliberately not persisted.

-- Content-addressed definition provenance: the exact definitions in effect
-- when a turn ran, enabling time travel over definition changes. Personas,
-- actors, and prompts are stored as full content snapshots (small markdown
-- files); MCP tools as descriptors only (server name and version from the
-- MCP initialize handshake, plus tool name, description, and inputSchema).
-- Content-addressing dedups: reverting a definition reuses the existing row.
CREATE TABLE definitions (
    hash          TEXT PRIMARY KEY,   -- SHA-256 of canonical content
    kind          TEXT NOT NULL,      -- persona | actor | prompt | mcp_tool
    name          TEXT NOT NULL,      -- definition name or namespaced invokable
    version       TEXT,               -- mcp_tool only: server-reported version
    content       TEXT NOT NULL,      -- definition bytes or tool descriptor JSON
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

-- Context assembly: walk the parent chain from the session head. A compaction
-- turn contributes its summary message, then either resumes from its
-- first_retained_turn_id (keep-recent: the retained range down to that turn is
-- included verbatim) or terminates (full compaction: nothing below the
-- summary). The summary sorts FIRST — it replaces the elided prefix. Messages
-- expand in order (chronological by turn, then position within turn).
WITH RECURSIVE walk AS (
    SELECT t.*, 0 AS depth, CAST(NULL AS TEXT) AS stop_at
    FROM turns t
    JOIN sessions s ON s.head_turn_id = t.id
    WHERE s.id = :session_id

    UNION ALL

    SELECT p.*, walk.depth + 1,
           CASE WHEN walk.kind = 'compaction'
                THEN walk.first_retained_turn_id
                ELSE walk.stop_at
           END
    FROM walk
    JOIN turns p ON p.id = walk.parent_id
    WHERE NOT (walk.kind = 'compaction' AND walk.first_retained_turn_id IS NULL)  -- full compaction: stop immediately
      AND (walk.stop_at IS NULL OR walk.id <> walk.stop_at)                       -- keep-recent: stop below the retained turn
)
SELECT m.id, m.turn_id, m.role, m.content, m.position, m.created_at, w.kind
FROM walk w
JOIN messages m ON m.turn_id = w.id
ORDER BY CASE WHEN w.kind = 'compaction' THEN 0 ELSE 1 END, w.depth DESC, m.position ASC;
