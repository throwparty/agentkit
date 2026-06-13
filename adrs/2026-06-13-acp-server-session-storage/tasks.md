---
status: draft
created: 2026-06-13
updated: 2026-06-13
author: adrian
decision: pending
---

# Tasks: ACP Server Session Storage

Build a standalone library crate (`acp-storage`) with three entities
implemented as vertical slices: Session → PromptTurn → Message, then
cross-entity features (context assembly, fork).

Each task is independently testable. Task 1 is the foundation. Tasks
2–4 each add one entity to both backends (InMemory + SQLite) with
its own migration. Task 5 completes the cross-entity operations.

## Summary

| # | Task | Effort | Depends on | Entity |
|---|------|--------|------------|--------|
| 1 | Scaffold + ID types + core types + trait | medium | — | Foundation |
| 2 | Session entity (both backends) | medium | 1 | Session |
| 3 | PromptTurn entity (both backends) | medium | 2 | PromptTurn |
| 4 | Message entity (both backends) | medium | 3 | Message |
| 5 | Context assembly + fork (both backends) | medium | 4 | Cross-entity |

Total: 5 tasks, estimated 1–3 days each.

---

## Task 1: Scaffold + ID types + core types + trait

**Effort**: medium · **Entity**: Foundation · **Depends on**: none

### Description

Create the `acp-storage` library crate at
`adrs/2026-06-13-acp-server-session-storage/` with:

1. **Cargo.toml** with dependencies (tokio, uuid, thiserror, async-trait,
   serde optional, sqlx optional behind `sqlite` feature).

2. **`src/id.rs`** — Three ID types as standalone structs (not an enum):

   - `SessionId` — encodes as `sess_<uuid>`, decodes by stripping `sess_`
   - `TurnId` — encodes as `turn_<uuid>`, decodes by stripping `turn_`
   - `MessageId` — encodes as `msg_<uuid>`, decodes by stripping `msg_`

   Each has:
   - `new()` — generates a random UUID v4
   - `from_uuid(bare: String)` — wraps an existing bare UUID
   - `encode(&self) -> String` — adds prefix
   - `decode(prefixed: &str) -> Result<String, IdError>` — strips prefix,
     validates it matches the expected type, returns bare UUID
   - `as_str(&self) -> &str` — returns bare UUID

   `IdError` with variants `InvalidFormat` and `WrongPrefix`.
   Bare UUIDs (no prefix) are accepted for backward compatibility.

3. **`src/types.rs`** — `Session`, `PromptTurn`, `Message` structs.
   All IDs are bare `String` fields (no ID wrappers). Includes
   `prompt_turn_count` on Session and fork lineage fields
   (`forked_from_session_id`, `fork_point_turn_id`).

4. **`src/store/mod.rs`** — `SessionStore` trait and `StoreError` enum.

   `StoreError`:
   ```rust
   pub enum StoreError {
       NotFound { entity: &'static str, id: String },
       AlreadyExists { entity: &'static str, id: String },
       Database(String),
   }
   ```

   `SessionStore` trait (all methods, some may remain unimplemented
   until later tasks):

   ```rust
   #[async_trait]
   pub trait SessionStore: Send + Sync {
       async fn create_session(&self, session: Session) -> Result<(), StoreError>;
       async fn get_session(&self, id: &str) -> Result<Session, StoreError>;
       async fn list_sessions(&self) -> Result<Vec<Session>, StoreError>;
       async fn close_session(&self, id: &str) -> Result<(), StoreError>;
       async fn set_session_mode(&self, id: &str, mode: String) -> Result<(), StoreError>;
       async fn set_session_head(&self, id: &str, head_prompt_turn_id: &str) -> Result<(), StoreError>;
       async fn append_prompt_turn(&self, turn: PromptTurn) -> Result<(), StoreError>;
       async fn get_prompt_turn_children(&self, id: &str) -> Result<Vec<PromptTurn>, StoreError>;
       async fn get_session_prompt_turns(&self, session_id: &str) -> Result<Vec<PromptTurn>, StoreError>;
       async fn append_message(&self, message: Message) -> Result<(), StoreError>;
       async fn get_messages_for_turn(&self, turn_id: &str) -> Result<Vec<Message>, StoreError>;
       async fn get_context(&self, session_id: &str, max_turns: Option<usize>) -> Result<Vec<Message>, StoreError>;
       async fn fork_session(&self, new_session: Session, source_session_id: &str, fork_point_turn_id: &str) -> Result<(), StoreError>;
       async fn clear(&self) -> Result<(), StoreError>;
   }
   ```

5. **Module wiring** in `src/lib.rs`:

   ```rust
   pub mod id;
   pub mod store;
   pub mod types;
   ```

### Tests (inline in `src/id.rs`)

| Test name | What it validates |
|-----------|-------------------|
| `session_id_new_roundtrip` | Create new SessionId, encode, decode back to original UUID |
| `session_id_from_uuid` | Wrap bare UUID, encode produces "sess_<uuid>", decode strips |
| `session_id_wrong_prefix_turn` | Decoding `turn_xxx` as SessionId → `IdError::WrongPrefix` |
| `session_id_wrong_prefix_msg` | Decoding `msg_xxx` as SessionId → `IdError::WrongPrefix` |
| `session_id_bare_uuid_accepted` | Decoding `abc` (no prefix) → `Ok("abc")` |
| `session_id_empty_rejected` | Decoding `""` → `IdError::InvalidFormat` |
| `session_id_prefix_only_rejected` | Decoding `"sess_"` → `IdError::InvalidFormat` |
| `turn_id_roundtrip` | Same for TurnId |
| `turn_id_decode_as_session_rejected` | Decoding `turn_xxx` via SessionId::decode → error |
| `message_id_roundtrip` | Same for MessageId |
| `message_id_decode_as_session_rejected` | Decoding `msg_xxx` via SessionId::decode → error |

### Acceptance criteria

1. `cargo build` succeeds for the new crate (no binary).
2. `cargo test` passes — all ID prefix tests green.
3. The crate compiles without sqlx or serde features enabled.
4. `cargo test --features sqlite,serde` also compiles.

---

## Task 2: Session entity — both backends

**Effort**: medium · **Entity**: Session · **Depends on**: 1

### Description

Implement session create, get, list, close, set_mode, set_head for both
backends. Each backend gets its own file. The in-memory one is complete;
the SQLite one also creates migration 001.

### InMemorySessionStore

File: `src/store/memory.rs`

```rust
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}
```

Implement:

| Method | Behavior |
|--------|----------|
| `create_session` | Insert session into HashMap. Return AlreadyExists if ID taken. |
| `get_session` | Return cloned session with prompt_turn_count = 0 (turns populated later by get_context). Return NotFound if missing. |
| `list_sessions` | Return all sessions cloned, each with prompt_turn_count = 0. |
| `close_session` | Set `active = false`, update `updated_at`. Return NotFound if missing. |
| `set_session_mode` | Set `mode`, update `updated_at`. Return NotFound if missing. |
| `set_session_head` | Set `head_prompt_turn_id`, update `updated_at`. Return NotFound if missing. |
| All other trait methods | Return `Err(StoreError::Database("not implemented"))` |

The `clear()` method drops and recreates the HashMap.

### SqliteSessionStore

File: `src/store/sqlite.rs`

```rust
pub struct SqliteSessionStore {
    pool: sqlx::SqlitePool,
}
```

- Constructor: `pub async fn connect(path: &str) -> Result<Self, StoreError>`
- Connects to the path (`:memory:` for tests, file path for persistence)
- Runs migrations (currently only 001 exists)
- Migration: `migrations/001_sessions.sql`

Migration 001:

```sql
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
```

Implement the same session methods as InMemorySessionStore, using sqlx.

Error mapping for sqlx errors:
- `RowNotFound` → `StoreError::NotFound { entity: "session" }`
- `Database(SQLITE_CONSTRAINT_UNIQUE)` → `StoreError::AlreadyExists`
- Any other `Database` → `StoreError::Database`

### Module updates

- `src/store/mod.rs` — add `mod memory;` and `mod sqlite;` (latter behind `#[cfg(feature = "sqlite")]`)
- `src/lib.rs` — no changes needed (already declares `pub mod store`)

### Tests

All tests run against both backends via a shared function:

```rust
// In src/store/memory.rs
#[cfg(test)]
mod tests {
    use crate::store::memory::InMemorySessionStore;
    use super::run_session_tests; // from test_helpers

    #[tokio::test]
    async fn test_session_crud() {
        let store = InMemorySessionStore::new();
        run_session_tests(&store).await;
    }
}

// In src/store/sqlite.rs
#[cfg(test)]
mod tests {
    use crate::store::sqlite::SqliteSessionStore;
    use super::run_session_tests;

    #[tokio::test]
    async fn test_session_crud() {
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_session_tests(&store).await;
    }
}
```

#### Session test scenarios (in `src/test_helpers.rs`)

```rust
pub async fn run_session_tests<S: SessionStore>(store: &S);
```

| Scenario | Steps | Expects |
|----------|-------|---------|
| `session_create_and_get` | Create session, get it | Same session returned, fields match |
| `session_create_duplicate` | Create same session twice | Second create → `AlreadyExists` |
| `session_list` | Create 3 sessions, list | List contains exactly 3 |
| `session_get_missing` | Get nonexistent ID | `NotFound` |
| `session_close` | Create, close, get | `active == false`, `updated_at` updated |
| `session_close_missing` | Close nonexistent ID | `NotFound` |
| `session_set_mode` | Create, set mode to "ask" | `mode == Some("ask")` |
| `session_set_head` | Create, set head to "some-turn-id" | `head_prompt_turn_id == "some-turn-id"` |
| `session_clear` | Create several, clear, list | List is empty |

### Acceptance criteria

1. All session test scenarios pass against `InMemorySessionStore`.
2. All session test scenarios pass against `SqliteSessionStore` with `:memory:`.
3. `SqliteSessionStore::connect(":memory:")` creates tables without error.
4. Duplicate session IDs produce `StoreError::AlreadyExists` on both backends.

---

## Task 3: PromptTurn entity — both backends

**Effort**: medium · **Entity**: PromptTurn · **Depends on**: 2

### Description

Add prompt turn operations to both backends. Add migration 002 for SQLite.

### Store changes

#### InMemorySessionStore additions

Add an internal `HashMap<String, PromptTurn>` alongside the sessions map.

```rust
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    prompt_turns: Arc<RwLock<HashMap<String, PromptTurn>>>,
}
```

Implement:

| Method | Behavior |
|--------|----------|
| `append_prompt_turn` | Insert prompt turn. Validate session exists (NotFound if missing). Track parent_id correctly. |
| `get_prompt_turn_children` | Return all turns whose `parent_id` matches the given ID. |
| `get_session_prompt_turns` | Return all turns with matching `session_id`, ordered by `position`. |

All previously-implemented session methods unchanged.

#### SqliteSessionStore additions

Migration 002 (`migrations/002_prompt_turns.sql`):

```sql
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
```

Implement the same three methods via SQL queries on pool.

### Test scenarios (add to `test_helpers.rs`)

```rust
pub async fn run_prompt_turn_tests<S: SessionStore>(store: &S);
```

| Scenario | Steps | Expects |
|----------|-------|---------|
| `prompt_turn_append` | Create session, append turn | Turn stored, retrievable |
| `prompt_turn_append_first` | Append turn to session with no existing turns | Succeeds (head NULL) |
| `prompt_turn_append_missing_session` | Append turn with bad session_id | `NotFound` |
| `prompt_turn_dag_parent` | Append turn A, append turn B with parent=A | B.parent_id == A.id |
| `prompt_turn_children` | Append A, B (child of A), C (child of A) | get_children(A) returns [B, C] |
| `prompt_turn_session_list` | Append turns to session, get_session_prompt_turns | Returns in position order |
| `prompt_turn_position_increments` | Append 3 turns | Positions 0, 1, 2 |

### Acceptance criteria

1. All prompt turn scenarios pass against both backends.
2. Existing session CRUD tests still pass (regression).
3. `get_prompt_turn_children` returns only direct children.
4. Appending with a nonexistent session_id returns `NotFound`.

---

## Task 4: Message entity — both backends

**Effort**: medium · **Entity**: Message · **Depends on**: 3

### Description

Add message operations to both backends. Add migration 003 for SQLite.

### Store changes

#### InMemorySessionStore additions

Add an internal `HashMap<String, Message>` alongside sessions and prompt_turns.

```rust
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    prompt_turns: Arc<RwLock<HashMap<String, PromptTurn>>>,
    messages: Arc<RwLock<HashMap<String, Message>>>,
}
```

Implement:

| Method | Behavior |
|--------|----------|
| `append_message` | Insert message. Validate prompt_turn exists (NotFound if missing). |
| `get_messages_for_turn` | Return all messages with matching `prompt_turn_id`, ordered by `position`. |

#### SqliteSessionStore additions

Migration 003 (`migrations/003_messages.sql`):

```sql
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
```

### Test scenarios (add to `test_helpers.rs`)

```rust
pub async fn run_message_tests<S: SessionStore>(store: &S);
```

| Scenario | Steps | Expects |
|----------|-------|---------|
| `message_append` | Create session + turn, append message | Message stored |
| `message_append_missing_turn` | Append message with bad turn_id | `NotFound` |
| `message_get_by_turn` | Append 3 messages to same turn, get_messages_for_turn | Returns 3 in position order |
| `message_position_order` | Append messages with positions 2, 0, 1 | get returns in order 0, 1, 2 |
| `message_multiple_turns` | Append messages to two different turns | get_messages_for_turn returns correct subset |

### Acceptance criteria

1. All message scenarios pass against both backends.
2. Existing session + prompt turn tests still pass.
3. Appending to a nonexistent prompt_turn returns `NotFound`.
4. Messages for a turn are returned in position order.

---

## Task 5: Context assembly + fork — both backends

**Effort**: medium · **Entity**: Cross-entity · **Depends on**: 4

### Description

Implement the two cross-entity operations: context assembly (walking the
prompt turn DAG to produce the message history) and session forking
(creating a new session that shares ancestor turns).

### Context assembly (`get_context`)

**InMemorySessionStore**:

```
1. Load session → get head_prompt_turn_id
2. Walk parent_id from head to root, collecting turn IDs in reverse
   (root first, head last)
3. For each turn ID, collect messages sorted by position
4. Flatten into Vec<Message>
5. Apply max_turns: take only the last N turns (closest to head)
```

**SqliteSessionStore** (recursive CTE):

```sql
WITH RECURSIVE turn_chain AS (
    SELECT id, parent_id, position, 1 AS depth
    FROM prompt_turns
    WHERE id = (SELECT head_prompt_turn_id FROM sessions WHERE id = ?)
    UNION ALL
    SELECT pt.id, pt.parent_id, pt.position, tc.depth + 1
    FROM prompt_turns pt
    JOIN turn_chain tc ON tc.parent_id = pt.id
)
SELECT m.id, m.prompt_turn_id, m.role, m.content, m.position, m.created_at
FROM turn_chain tc
JOIN messages m ON m.prompt_turn_id = tc.id
ORDER BY tc.depth DESC, m.position ASC
LIMIT ?;
```

### Fork (`fork_session`)

**Both backends**:

```
1. Validate source session exists (NotFound if missing)
2. Validate fork_point_turn exists (NotFound if missing)
3. Insert new session with:
   - id = new session ID
   - head_prompt_turn_id = fork_point_turn_id
   - forked_from_session_id = source session ID
   - fork_point_turn_id = fork_point_turn_id
   - other fields from new_session parameter
4. Return Ok(())
```

No turns or messages are copied. The new session's context query walks
the existing DAG, naturally including the source session's ancestor turns.

### Test scenarios (add to `test_helpers.rs`)

#### Context assembly

```rust
pub async fn run_context_tests<S: SessionStore>(store: &S);
```

| Scenario | Setup | Expects |
|----------|-------|---------|
| `context_linear_chain` | Session with 3 turns (A→B→C), each with 2 messages | Context returns [A1, A2, B1, B2, C1, C2] |
| `context_empty_session` | Session with no turns | Empty vec |
| `context_max_turns` | Session with 5 turns, max_turns=3 | Returns messages from turns 3,4,5 only |
| `context_no_max_turns` | Session with 5 turns, max_turns=None | Returns messages from all 5 turns |
| `context_single_message_turn` | Turn with 1 message | Returns that message |

#### Fork

```rust
pub async fn run_fork_tests<S: SessionStore>(store: &S);
```

| Scenario | Setup | Expects |
|----------|-------|---------|
| `fork_session_basic` | Session A with turns, fork to Session B | B.head_prompt_turn_id == A's head |
| `fork_context_includes_ancestors` | Fork B from A, get_context for B | Same messages as A's context |
| `fork_preserves_source` | Fork B from A, add turn to B | A's context unchanged |
| `fork_independent_evolution` | Fork B from A, add turns to both | Each has independent context after fork |
| `fork_nonexistent_source` | Fork from bad session_id | `NotFound` |
| `fork_list_children` | Fork B and C from A | Query indexed by forked_from_session_id |

### Acceptance criteria

1. Context assembly returns messages in chronological order (root turn
   messages first, then children).
2. `max_turns=N` limits the output to the most recent N turns.
3. Empty session returns empty context.
4. Forked session's context includes ancestor messages from source session.
5. Forked and source sessions evolve independently after fork.
6. All existing entity-level tests still pass.

---

## Dependencies

```
Task 1 (scaffold + IDs + types + trait)
  │
  ▼
Task 2 (Session: InMemory + SQLite)
  │
  ▼
Task 3 (PromptTurn: InMemory + SQLite)
  │
  ▼
Task 4 (Message: InMemory + SQLite)
  │
  ▼
Task 5 (Context assembly + fork: InMemory + SQLite)
```

No horizontal layering — each task delivers a complete, testable entity
across both backends.
