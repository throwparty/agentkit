CREATE TABLE IF NOT EXISTS models (
    id                  TEXT PRIMARY KEY,
    context_window      INTEGER,
    max_output          INTEGER,
    tool_calling        INTEGER,
    reasoning           INTEGER,
    structured_output   INTEGER,
    synced_at           INTEGER NOT NULL
);
