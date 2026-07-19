CREATE TABLE IF NOT EXISTS session_affinity (
    session_id          TEXT PRIMARY KEY,
    provider_identity   TEXT NOT NULL,
    model_name          TEXT NOT NULL,
    api_surface         TEXT NOT NULL DEFAULT 'openai',
    assigned_at         INTEGER NOT NULL,
    last_used_at        INTEGER NOT NULL,
    total_input_tokens  INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_requests      INTEGER DEFAULT 0,
    switch_count        INTEGER DEFAULT 0,
    is_active           INTEGER DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_affinity_last_used ON session_affinity(last_used_at);
CREATE INDEX IF NOT EXISTS idx_affinity_provider ON session_affinity(provider_identity);

CREATE TABLE IF NOT EXISTS routing_events (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id          TEXT,
    request_id          TEXT NOT NULL,
    model_name          TEXT NOT NULL,
    provider_identity   TEXT NOT NULL,
    billing_model       TEXT NOT NULL,
    decision_reason     TEXT NOT NULL,
    input_tokens        INTEGER,
    output_tokens       INTEGER,
    response_status     INTEGER,
    latency_ms          INTEGER,
    degraded_providers  TEXT,
    created_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_routing_session ON routing_events(session_id);
CREATE INDEX IF NOT EXISTS idx_routing_time ON routing_events(created_at);

CREATE TABLE IF NOT EXISTS credential_meta (
    identity        TEXT PRIMARY KEY,
    auth_type       TEXT NOT NULL,
    source          TEXT NOT NULL,
    expires_at      INTEGER,
    refresh_enabled INTEGER DEFAULT 1,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
