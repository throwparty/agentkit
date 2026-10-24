# store

Session storage: the turn DAG and the messages behind a single
`SessionStore`, with SQLite (durable) and in-memory (tests) backends.

## What lives here

```
mod.rs       module doc + re-exports
model.rs     the data model: Session, Turn, Message, ids, kinds,
             TurnUsage, AssembledMessage, ListFilter, StoreError
session.rs   SessionStore and its operations; dispatches to the SQLite
             pool or the in-memory backend
sqlite.rs    SQLite row mapping (SqliteSession, SqliteAssembledRow) and
             file-permission helpers
assembly.rs  the compaction truncation state machine's ordering step,
             shared by the in-memory backend, the daggy mirror, and tests
memory.rs    the in-memory backend (test-only)
graph.rs     the daggy structural mirror (test-only for now: a future
             session-tree service, awaiting an ACP interface to expose it)
tests.rs     backend parity, migrations, leases, soft delete, and the
             daggy assembly oracle
migrations/  the SQLite schema
```

SQLite is the source of truth; the daggy graph is a rebuildable mirror.
The in-memory backend is the readable reference the SQL queries are
tested against, and the graph is the independent oracle for the assembly
walk.

## Dependencies

- depends on: `sqlx` (SQLite), `uuid`, `serde_json`
- depended on by: `agent` (turn persistence), `providers` (test fixtures),
  `scripts` (history search, fork, compaction recording), `acp::v1`
  (session lifecycle)
