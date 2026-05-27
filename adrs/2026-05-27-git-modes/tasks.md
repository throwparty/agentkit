---
status: planning
created: 2026-05-27
updated: 2026-06-04
author: adrian
---

# ADR: git-modes — Implementation Tasks

## Task 1: VcsStore Path Resolution Module

**Summary:** Implement platform-specific VcsStore path resolution using `std::env::consts::OS`.

**Relevant spec/plan sections:** §3.1, §2.2

**Acceptance criteria:**
- `VcsStore::resolve_path()` returns `~/Application Support/AgentKit/Litterbox/vcs/git/<slug>` on macOS
- `VcsStore::resolve_path()` returns `~/.local/state/agentkit/litterbox/vcs/git/<slug>` on Linux/default
- `VcsStore::resolve_path()` returns `~/AppData/LocalLow/AgentKit/Litterbox/vcs/git/<slug>` on Windows
- Path is deterministic from project slug (basename of VCS root if no explicit slug)

**Required tests:**
- Unit test: explicit slug → correct path
- Unit test: default slug (basename) → correct path
- Unit test: platform detection via `std::env::consts::OS`

**Dependencies:** None (first task)

**Rollback:** N/A (new module)

---

## Task 2: VcsStore Bare Clone Creation (Shallow + Self-Healing)

**Summary:** Implement bare clone creation with `--depth 1` and self-healing.

**Relevant spec/plan sections:** §3.1 (clone_bare), §2.3, §4.1

**Acceptance criteria:**
- `clone_bare()` creates bare repository at resolved path
- Clone uses `--depth 1` (shallow)
- `git2::Repository::clone_with()` is used for shallow clone
- Self-healing: destroy bare clone, re-clone on error detection
- No duplicate clones: if path exists, skip clone

**Required tests:**
- Unit test: shallow clone produces `refs/heads/litterbox/*` branch
- Unit test: existing path skips clone
- Integration test: clone path matches resolved path

**Dependencies:** Task 1 (path resolution must exist)

**Rollback:** N/A (new module)

---

## Task 3: VcsStore Remote Installation/Removal

**Summary:** Implement `litterbox` remote installation, removal, and conflict detection.

**Relevant spec/plan sections:** §3.1 (install_remote, remove_remote), §4.1

**Acceptance criteria:**
- `install_remote()` adds `litterbox` remote pointing to bare clone path
- `install_remote()` is idempotent (second call is no-op)
- `install_remote()` errors if remote URL differs
- `remove_remote()` removes `litterbox` remote if exists
- **Remote is NEVER removed during sandbox delete** (the remote persists indefinitely)

**Required tests:**
- Unit test: first install succeeds, adds `litterbox` remote
- Unit test: second install is no-op (skips if exists)
- Unit test: conflicting URL returns error
- Unit test: remove_remote removes `litterbox` remote
- Integration test: multiple sandboxes share same remote

**Dependencies:** Task 2 (bare clone must exist)

**Rollback:** N/A (new module)

---

## Task 4: GitScm Mode-Aware Implementation

**Summary:** Extend `GitScm` with mode field, implement remote mode paths, maintain direct mode unchanged.

**Relevant spec/plan sections:** §3.2, §4.1, §4.2

**Acceptance criteria:**
- `GitScm` struct has `mode: ScmMode` field (`Direct` or `Remote`)
- `GitScm::new(mode, path)` sets mode correctly
- Remote mode: `commit_snapshot()` operates on bare clone, not user's repo
- Direct mode: `commit_snapshot()` operates on user's repo (unchanged)
- Remote mode: `head_commit()` reads from bare clone
- Direct mode: `head_commit()` reads from user's repo (unchanged)
- `snapshot_branch` field stores sandbox branch name

**Required tests:**
- Unit test: remote mode commits to bare clone path
- Unit test: direct mode commits to user's repo (unchanged)
- Unit test: head_commit reads from correct repo based on mode
- Integration test: mode binding works (existing sandboxes keep their mode)

**Dependencies:** Task 2 (bare clone creation), Task 3 (remote installation)

**Rollback:** N/A (additive change, no breaking changes)

**Revisions:**
- Added `host_repo_path: Option<PathBuf>` to GitScm for remote mode. Required because `make_archive("HEAD")` must read from the host repo (the bare clone's HEAD is the initial shallow clone commit, not the user's working tree). Initial implementation omitted this entirely.
- `head_commit` ACs were specified but not implemented — deferred as not critical for the initial flow.

---

## Task 5: SandboxMetadata Extension

**Summary:** Extend `SandboxMetadata` with `mode` and `project_slug` fields.

**Relevant spec/plan sections:** §3.3

**Acceptance criteria:**
- `SandboxMetadata` has `mode: ScmMode` field
- `SandboxMetadata` has `project_slug: String` field
- Existing sandboxes get `Direct` for mode (`project_slug` derived from config at runtime)
- New sandboxes get mode from config and `project_slug` from `repo_prefix()`
- Serialization/deserialization includes new fields (backward compatible)

**Required tests:**
- Unit test: metadata serialization includes new fields
- Unit test: existing metadata (without new fields) deserials correctly
- Integration test: new sandboxes have `Remote` mode and `project_slug` populated
- Integration test: old sandboxes have `Direct` mode

**Dependencies:** Task 4 (GitScm mode field)

**Rollback:** N/A (additive change)

**Revisions:**
- Initial implementation stored `bare_clone_path` (a filesystem path). Changed to `project_slug` (an identifier) to decouple compute from VCS path layout.

---

## Task 5a: SandboxMetadata Persistence (MetadataStore)

**Summary:** Add `MetadataStore` struct for persisting `SandboxMetadata` to TOML files with `flock`-based locking. Add `store_metadata`, `load_metadata`, `remove_metadata` methods to the `Scm` trait. Wire into `create`, `resolve_sandbox_metadata`, and `delete`.

**Relevant spec/plan sections:** §5.4, §2.4, §3.5

**Acceptance criteria:**
- `MetadataStore::store()` writes a valid TOML file containing all `SandboxMetadata` fields
- `MetadataStore::load()` returns `None` for missing files (legacy backward compat)
- `MetadataStore::store()` uses `flock(LOCK_EX)` and retries on `EAGAIN` with exponential backoff (50ms, 150ms, 350ms)
- `Scm` trait gains `store_metadata`, `load_metadata`, `remove_metadata` methods
- `GitScm` delegates to `MetadataStore` using its own `project_slug` field
- `ThreadSafeScm` delegates to inner `GitScm`
- `DockerSandboxProvider::create()` calls `scm.store_metadata()` after successful creation
- `resolve_sandbox_metadata()` calls `scm.load_metadata()` instead of reconstructing metadata with hardcoded `Direct` mode
- `DockerSandboxProvider::delete()` calls `scm.remove_metadata()`
- `TestScm` implements the three new methods as no-ops

**Required tests:**
- Unit test: metadata file written and read back matches
- Unit test: missing metadata file returns `None` (legacy compat)
- Unit test: concurrent `store` from two processes serializes via `flock` (slow, mark as integration)
- Unit test: `EAGAIN` retry logic succeeds after transient lock contention
- Integration test: end-to-end mode binding survives config change

**Dependencies:** Task 5 (SandboxMetadata fields exist)

**Rollback:** N/A (additive change; existing sandboxes without metadata files fall back to legacy Direct mode)

---

## Task 6: Configuration Wiring

**Summary:** Wire `snapshot-mode` into configuration, parse from `[git]` section.

**Relevant spec/plan sections:** §3.4, §4.1

**Acceptance criteria:**
- `ProjectConfig` gains `snapshot_mode: Option<SnapshotMode>` field
- `[git].snapshot-mode` parses from `.litterbox.toml`
- Default mode is `Remote`
- `config_loader` merges `snapshot-mode` correctly
- Mode is stored in `SandboxMetadata` at creation time

**Required tests:**
- Unit test: config parsing covers `direct`, `remote`, and missing (default)
- Unit test: default mode is `Remote`
- Integration test: config is passed to `GitScm::open()`
- Integration test: sandbox creation respects config mode

**Dependencies:** Task 5 (SandboxMetadata extension)

**Rollback:** N/A (additive change)

**Revisions:**
- Initial implementation parsed `snapshot-mode` and threaded the mode value to `ThreadSafeScm::open_with_mode_and_prefix()`, but `open_with_mode_and_prefix()` was called on `"."` — the user's repo, not the bare clone. No task specified that when mode is `Remote`, the SCM should be opened on the bare clone path instead. The VcsStore operations (clone_bare, install_remote) were never called from `build_provider_with_config()`. This was the largest integration gap across all tasks.

---

## Task 6a: VcsStore → Provider Wiring (Glue)

**Summary:** Wire `build_provider_with_config()` to call VcsStore operations and open the SCM on the bare clone when mode is `Remote`. This is the critical integration step that connects the independent modules.

**Relevant spec/plan sections:** §4.1 (data flow), §1.2

**Acceptance criteria:**
- When `config.git.snapshot_mode == Remote`, `build_provider_with_config()`:
  1. Computes the project slug from config or current directory basename.
  2. Calls `VcsStore::clone_bare(host_path, slug)` — creates bare clone if missing.
  3. Calls `VcsStore::install_remote(host_path, bare_path)` — adds `litterbox` remote.
  4. Opens `GitScm::open_with_host(bare_path, Remote, Some(host_abs))` — SCM on bare clone with host path for archive reads.
- When `snapshot_mode == Direct`, opens SCM on `"."` (unchanged).
- `make_archive("HEAD")` reads from host repo via `host_repo_path` when mode is `Remote`.
- `create_branch()` creates the branch inside the bare clone, NOT the user's repo.

**Required tests:**
- Integration test: `remote_mode_operations_use_bare_clone_not_host` validates the full pipeline — branch in bare clone only, archive reads from host, `litterbox` remote installed, delete cleans up, remote persists.

**Dependencies:** Tasks 1-3 (VcsStore), Task 4 (GitScm mode), Task 6 (config)

**Rollback:** N/A (additive change)

**Note:** This task was originally missing from the task list. The plan's data flow (§4.1 steps 5-7) describes the wiring in prose, but no task translated it to code. The initial implementation treated Tasks 1-3 and 4-6 as independent chains — the VcsStore module and the config/GitScm extensions were never connected. This gap caused remote mode to be a complete no-op (SCM opened on user's repo regardless of mode; clone_bare and install_remote never called).

---

## Task 7: Delete Logic Update (Remote Mode)

**Summary:** Update `delete()` to handle remote mode — remove branch, keep remote, optionally remove bare clone.

**Relevant spec/plan sections:** §4.3

**Acceptance criteria:**
- `delete()` checks `SandboxMetadata.mode` for mode detection
- Remote mode: removes branch from bare clone
- Remote mode: **does NOT remove `litterbox` remote** (the remote is never removed automatically)
- Remote mode: removes bare clone only if last sandbox for project
- Direct mode: unchanged behavior

**Required tests:**
- Unit test: remote mode delete removes branch from bare clone
- Unit test: remote mode delete does NOT remove `litterbox` remote (persists even on last sandbox)
- Integration test: multiple sandboxes share remote, delete one keeps remote
- Integration test: delete removes bare clone when last sandbox for project

**Dependencies:** Task 4 (GitScm mode), Task 6 (configuration)

**Rollback:** N/A (additive change)

---

## Task 8: Integration Tests & Verification

**Summary:** End-to-end tests covering both modes, mode binding, and error recovery.

**Relevant spec/plan sections:** §5.3

**Acceptance criteria:**
- AC-1 through AC-14 all verified
- `sandbox-create` → `write` → `commit_snapshot` → `delete` works end-to-end for both modes
- Concurrent clones serialize correctly (no race condition)
- Self-healing works (corrupt clone → re-clone → works)
- Direct mode unchanged (existing tests pass)
- Remote mode works (new tests pass)

**Required tests:**
- Integration test: remote mode end-to-end flow
- Integration test: direct mode end-to-end flow (unchanged)
- Integration test: concurrent `sandbox-create` for same project (one clone)
- Integration test: mode binding (existing sandboxes keep their mode)
- Integration test: self-healing (corrupt clone → re-clone)
- Integration test: `litterbox` remote persists after sandbox delete

**Dependencies:** Tasks 1–7 (all components must exist)

**Rollback:** N/A (tests only)

---

## Task 9: Documentation & Rollout

**Summary:** Update documentation, verify rollout strategy, ensure backward compatibility.

**Relevant spec/plan sections:** §6, §7

**Acceptance criteria:**
- `README.md` documents `snapshot-mode` configuration
- `CONFIG.md` (or equivalent) documents new `[git]` section
- Migration guide documents direct → remote transition
- All acceptance criteria (AC-1 to AC-14) pass in CI
- Direct mode regression tests pass

**Required tests:**
- Documentation review (manual)
- CI pipeline: all tests pass (unit + integration)
- Manual test: direct mode unchanged (regression)
- Manual test: remote mode works end-to-end

**Dependencies:** Task 8 (all tests)

**Rollback:** N/A (documentation only)

---

## Task Dependencies Graph

```
Task 1 (VcsStore path) → Task 2 (bare clone) → Task 3 (remote install)
                                                       ↓
Task 4 (GitScm mode) ──────────────────────────→ Task 6a (PROVIDER WIRING) ←── Task 6 (config)
                                                       │
                                                       ├──→ Task 5 (metadata struct) → Task 5a (MetadataStore)
                                                       │                                           ↓
                                                       ├──→ Task 7 (delete) ←────────────────────┘
                                                       └──→ Task 8 (integration) → Task 9 (docs)
```

## Rollout Strategy

1. **Phase 1 (Tasks 1–3):** Implement VcsStore module (path resolution, bare clone, remote install).
2. **Phase 2 (Tasks 4–6):** Extend GitScm, SandboxMetadata, configuration wiring.
3. **Phase 2a (Task 5a):** MetadataStore — persist metadata, add Scm trait methods, wire resolve.
4. **Phase 3 (Task 7):** Update delete logic for remote mode.
5. **Phase 4 (Task 8):** Integration tests and verification.
6. **Phase 5 (Task 9):** Documentation and rollout.

Each phase is independently deployable; direct mode remains unchanged throughout.

---

## Acceptance Criteria Traceability

| AC | Task | Status |
|----|------|--------|
| AC-1: Direct mode branches in user's repo | Task 4, 6, 6a, 8 | Implemented |
| AC-2: Remote mode bare clone at resolved path | Task 1, 2, 6, 6a | Implemented |
| AC-3: Remote mode branch exists with valid commit | Task 2, 4, 6a, 8 | Implemented (verified by reproducer test) |
| AC-4: Remote mode commit_snapshot_from_staging works | Task 4, 6a, 8 | Implemented |
| AC-5: Remote mode litterbox remote exists | Task 3, 6a, 8 | Implemented (verified by reproducer test) |
| AC-6: Remote mode no re-clone for same project | Task 2, 8 | Implemented |
| AC-7: Remote mode self-heals corrupt clone | Task 2, 8 | Implemented |
| AC-8: Remote mode metadata includes project slug | Task 5, 6a, 8 | Implemented |
| AC-9: Remote mode delete removes bare clone (last sandbox); remote persists | Task 7, 8 | Implemented |
| AC-10: Remote mode shallow clone | Task 2, 8 | Implemented |
| AC-11: Remote mode remote idempotent install | Task 3, 8 | Implemented |
| AC-12: Remote mode project slug resolves correctly | Task 1, 6, 6a, 8 | Implemented |
| AC-13: Remote mode rebases don't break fetch/merge | Task 4, 8 | Not verified (manual test) |
| AC-14: Remote mode GC-pruned root recovers | Task 2, 8 | Implemented (self_heal) |
| AC-15: Mode binding across config changes | Task 5a, 8 | New |
| AC-16: Legacy fallback for absent metadata | Task 5a, 8 | New |
