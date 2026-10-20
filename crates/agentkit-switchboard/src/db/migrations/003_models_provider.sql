ALTER TABLE models RENAME TO models_legacy;

CREATE TABLE models (
    id                  TEXT NOT NULL,
    provider            TEXT NOT NULL,
    context_window      INTEGER,
    max_output          INTEGER,
    tool_calling        INTEGER,
    reasoning           INTEGER,
    structured_output   INTEGER,
    synced_at           INTEGER NOT NULL,
    PRIMARY KEY (id, provider)
);

DROP TABLE models_legacy;
