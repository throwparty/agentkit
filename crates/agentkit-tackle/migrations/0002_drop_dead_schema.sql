-- Migration 0002: drop schema that was adopted from the storage ADR but
-- never wired up. The `definitions`/`turn_definitions` tables (per-turn
-- definition-version tracking) have no reader or writer in the crate, and
-- the `metadata` columns on `turns` and `messages` are likewise never
-- read or written — only `sessions.metadata` is used.

DROP INDEX IF EXISTS idx_turn_definitions_hash;
DROP TABLE IF EXISTS turn_definitions;
DROP TABLE IF EXISTS definitions;

ALTER TABLE turns DROP COLUMN metadata;
ALTER TABLE messages DROP COLUMN metadata;
