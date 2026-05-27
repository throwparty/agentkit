---
status: planning
created: 2026-05-27
updated: 2026-06-04
author: adrian
---

# ADR: git-modes — Technical Plan

## 1. Architecture Overview

The implementation introduces a **remote mode** alongside the existing **direct mode** through four new components and two extensions to existing ones. All components are implemented in Rust using `git2`, with no external dependencies.

### 1.1 VcsStore — Bare Clone Lifecycle Manager
- **Responsibility**: Manages the filesystem-backed bare clone store at `~/.local/state/agentkit/litterbox/vcs/git/<project-slug>/`.
- **Operations**: Path resolution, bare clone creation/destruction, `litterbox` remote installation, self-healing.
- **Interface**:
  - `resolve_path(project_slug: &str) -> PathBuf`: Returns platform-specific VcsStore root.
  - `clone_bare(host_path: &Path) -> Result<Path, SandboxError>`: Creates bare clone with `--depth 1`.
  - `install_remote(repo_path: &Path, bare_path: &Path) -> Result<(), SandboxError>`: Installs `litterbox` remote if absent.
  - `remove_remote(repo_path: &Path) -> Result<(), SandboxError>`: Removes `litterbox` remote.
  - `self_heal(host_path: &Path, bare_path: &Path) -> Result<(), SandboxError>`: Destroys and re-clone on error.

### 1.2 GitScm (Mode-Aware)
- **Responsibility**: All SCM operations operate on either the user's `.git` (direct) or bare clone (remote).
- **Interface unchanged**: `Scm` trait methods are identical across modes. Mode is an internal parameter.
- **Mode binding**: `GitScm::open(path, mode)` sets the mode. `commit_snapshot()`, `delete_branch()`, `head_commit()` operate on the appropriate repository.
- **Archive reads from host**: `make_archive("HEAD")` must read from the host repo even in remote mode (the bare clone's HEAD is the initial clone, not the user's working tree). `GitScm` stores `host_repo_path: Option<PathBuf>` and opens a temporary `Repository` there for archive operations when mode is `Remote`.

### 1.3 SandboxMetadata Extension
- **Responsibility**: Stores mode and project slug per sandbox instance.
- **Fields added**:
  - `mode: ScmMode` — `Direct` or `Remote`
  - `project_slug: String` — set from `repo_prefix()` at creation, used to derive bare clone path via `VcsStore::resolve_path()`

### 1.4 Configuration Wiring
- **Responsibility**: Exposes `snapshot-mode` in `[git]` section.
- **Values**: `direct` (unchanged), `remote` (new, default).
- **Scope**: Per-project config. Mode is stored in `SandboxMetadata` at creation time and persisted via MetadataStore.

### 1.5 MetadataStore — Per-Sandbox Metadata Persistence
- **Responsibility**: Persists and loads `SandboxMetadata` to/from the filesystem.
- **Location**: `~/.local/state/agentkit/litterbox/metadata/<project-slug>/<sandbox-slug>.toml`
- **Backward compat**: Missing file → legacy Direct mode.

---

## 2. Technology Choices

### 2.1 `git2` for all operations
- Uses the `git2` crate already in the project.
- `git2::Repository::clone_with()` for shallow clone.
- `git2::Repository::find_remote()` / `create_remote()` for remote management.
- Justification: Zero new dependencies, consistent with existing codebase, provides full control over bare repo operations.

### 2.2 Platform detection: `std::env::consts::OS`
- Uses Rust's built-in `std::env::consts::OS` constant.
- Compile-time strings: `"macos"`, `"linux"`, `"windows"`.
- No external crates needed.
- Justification: Idiomatic Rust, no runtime overhead, deterministic output.

### 2.3 Per-sandbox metadata store (TOML files + flock)
- Each sandbox's `SandboxMetadata` is persisted to `~/.local/state/agentkit/litterbox/metadata/<project>/<slug>.toml`.
- TOML format is chosen because the `toml` crate is already a dependency.
- Advisory file locking (`flock` via `libc`) coordinates concurrent writers across MCP server instances.
- On read, if no file exists, the sandbox is assumed to be legacy Direct mode — metadata reconstructed by convention.
- Justification: No schema migrations (each file is self-describing); no coupling between compute and VCS layers; single-file-per-sandbox means no write contention across different sandboxes; `libc::flock` is used directly (libc is already a transitive dependency).

---

## 3. Component Breakdown

### 3.1 VcsStore Module
```
crates/agentkit-litterbox/src/vcs_store/
├── mod.rs          # VcsStore struct, resolve_path(), clone_bare()
├── path.rs         # Platform-specific path resolution
└── remote.rs       # Remote installation/removal logic
```

**VcsStore responsibilities:**
- `resolve_path(project_slug: &str) -> PathBuf`: Returns `VcsStore/git/<project-slug>/`.
- `clone_bare(host_path: &Path) -> Result<Path, SandboxError>`:
  - Opens host repo via `git2::Repository::open(host_path)`.
  - Creates `VcsStore/git/<project-slug>/` directory if missing.
  - Clones as bare repo (`bare=true`) with `--depth 1`.
  - Returns path.
- `install_remote(repo_path: &Path, bare_path: &Path) -> Result<(), SandboxError>`:
  - Opens repo at `repo_path`.
  - Checks if `litterbox` remote exists.
  - If not, adds `litterbox` → `bare_path`.
  - Returns error if URL differs.
   - `remove_remote(repo_path: &Path) -> Result<(), SandboxError>`:
     - Opens repo at `repo_path`.
     - Removes `litterbox` remote if exists (utility function; not called in automated delete — the remote is never removed automatically).
    - `self_heal(host_path: &Path, bare_path: &Path) -> Result<(), SandboxError>`:
  - On corrupt refs:
    - Delete `bare_path/`.
    - Re-clone with `--depth 1`.

### 3.2 GitScm (mode-aware)
```rust
pub struct GitScm {
    repo: Repository,
    mode: ScmMode,  // Direct or Remote
    snapshot_branch: Option<String>,
    host_repo_path: Option<PathBuf>,  // Remote mode only: points to user's working repo for archive reads
}
```

**Changes:**
- Add `mode: ScmMode` field.
- Add `host_repo_path: Option<PathBuf>` for remote mode — used by `make_archive()` to read the host repo's HEAD instead of the bare clone's HEAD.
- Modify `open(path, mode)` → `open(path, ScmMode)`, plus `open_with_host(path, mode, host_repo_path)` for remote mode.
- Modify `commit_snapshot()` to operate on bare clone (remote) or user's repo (direct).
- **CRITICAL**: `make_archive("HEAD")` opens the host repo temporarily when mode is Remote. The bare clone's HEAD is the initial clone commit, not the user's working tree — archive from the bare clone would produce the wrong content.

### 3.3 SandboxMetadata extension
```rust
pub struct SandboxMetadata {
    pub name: String,
    pub branch_name: String,
    pub container_id: String,
    pub status: SandboxStatus,
    pub mode: ScmMode,  // NEW: Direct or Remote
    pub project_slug: String,  // NEW: for path derivation
    pub forwarded_ports: Vec<ForwardedPortMapping>,
}
```

### 3.4 Configuration
```rust
pub struct ProjectConfig {
    pub slug: Option<String>,
    pub snapshot_mode: Option<SnapshotMode>,  // NEW
}

pub enum SnapshotMode {
    Direct,
    Remote,
}
```

### 3.5 MetadataStore Implementation
```rust
pub struct MetadataStore {
    base_path: PathBuf,
}

impl MetadataStore {
    /// Resolve path for a given project slug and sandbox slug.
    fn path_for(&self, project: &str, slug: &str) -> PathBuf;

    /// Write metadata with exclusive flock, retry on EAGAIN.
    fn store(&self, project: &str, slug: &str, meta: &SandboxMetadata) -> Result<(), SandboxError>;

    /// Load metadata. Returns None if file doesn't exist (legacy).
    fn load(&self, project: &str, slug: &str) -> Result<Option<SandboxMetadata>, SandboxError>;

    /// Delete metadata file.
    fn remove(&self, project: &str, slug: &str) -> Result<(), SandboxError>;
}
```

---

## 4. Data Flow

### 4.1 `sandbox-create` (remote mode)
1. MCP request arrives with `name: "my-feature"`.
2. `config_loader.load_final()` → `Config { project: { slug: None, snapshot_mode: Remote } }`.
3. `resolve_project_slug()` → `"myproject"` (from current directory basename).
4. `VcsStore::resolve_path("myproject")` → `~/.local/state/agentkit/litterbox/vcs/git/myproject/`.
5. `VcsStore::clone_bare(host_path)` → returns path (clones if missing).
6. `VcsStore::install_remote(host_path, bare_path)` → adds `litterbox` remote.
7. `GitScm::open_with_host(bare_path, ScmMode::Remote, host_path)` → opens bare clone, stores host path for archive reads.
8. `scm.create_branch("my-feature")` → creates `litterbox/my-feature` branch inside the bare clone.
9. `scm.make_archive("HEAD")` → reads from the host repo (via `host_repo_path`), not the bare clone. Required because the bare clone's HEAD is the initial shallow clone, not the user's working tree.
9a. `MetadataStore::store(project_slug, "my-feature", &metadata)` — persists mode, branch_name, project_slug, container_id.
10. Return `SandboxMetadata { name, branch_name, ..., mode: Remote, project_slug: "myproject" }`.

**Wiring responsibility:** `build_provider_with_config()` (in `mcp.rs`) orchestrates steps 2-7. It must:
1. Detect `config.git.snapshot_mode == Remote`.
2. Compute the project slug.
3. Call `VcsStore::clone_bare(host_path, slug)`.
4. Call `VcsStore::install_remote(host_path, bare_path)`.
5. Open `GitScm::open_with_host(bare_path, Remote, host_path)`.
6. For Direct mode, it opens `GitScm::open(".", Direct)` with no host path (unchanged behaviour).

### 4.2 `write` / `commit_snapshot` (remote mode)
1. Mutation happens in staging directory.
2. `GitScm::commit_snapshot_from_staging(staging_path, "message")` opens bare clone.
3. Creates full-tree commit on `litterbox/my-feature` branch.
4. No changes to user's repo.
5. **Note**: `commit_snapshot()` (no staging path) returns `Ok(None)` on bare repos because there is no workdir to snapshot. The snapshot path always goes through `commit_snapshot_from_staging()` in practice.

### 4.3 `delete` (remote mode)
1. `SandboxMetadata.mode == Remote`.
2. `GitScm::delete_branch("my-feature")` → removes branch from bare clone.
3. Check if last sandbox for project (via `list_sandboxes` → filter by `mode == Remote` + project).
4. If last:
   - `VcsStore::destroy_bare(project_slug)` → deletes `VcsStore/git/<project-slug>/`.
   - The `litterbox` remote in the user's repo is **not** removed — it persists across all sandbox deletions.

### 4.4 Metadata Lifecycle

1. `sandbox-create`: After the container is provisioned and the branch is created, `MetadataStore::store()` writes the file. If the write fails (disk full, lock timeout), the creation is aborted (container + branch rolled back).
2. `read`/`write`/`bash`/`patch`: `resolve_sandbox_metadata` calls `MetadataStore::load()`. If the file is absent, falls back to legacy Direct mode.
3. `delete`: `MetadataStore::remove()` is called after the SCM branch is removed. If the file cannot be deleted (permission error), the error is logged but does not block deletion.

### 4.5 Error recovery (remote mode)
1. `clone_bare()` opens the bare repo and calls `Repository::open()`. If the repo is corrupt (`Err`), calls `VcsStore::self_heal(slug)` → re-clone.
2. Retry failed operation.

---

## 5. Testing Strategy

### 5.1 Unit Tests
- **VcsStore path resolution**: Test `resolve_path()` for explicit slug, default slug.
- **Shallow clone**: Test `clone_bare()` produces `refs/heads/litterbox/*` branch.
- **Remote installation**: Test `install_remote()` idempotency (first call adds, second call no-op).
- **Remote utility**: Test `remove_remote()` removes `litterbox` remote (utility test; not exercised in the delete flow).
- **GitScm remote mode**: Test `commit_snapshot()` writes to bare clone, not user's repo.
- **GitScm direct mode**: Test `commit_snapshot()` writes to user's repo (unchanged).

### 5.2 Integration Tests
- **End-to-end remote mode**: `sandbox-create` → `write` → `commit_snapshot` → `delete` all work.
- **End-to-end direct mode**: `sandbox-create` → `write` → `commit_snapshot` → `delete` all work (unchanged).
- **Concurrent clones**: Two `sandbox-create` for same project → one clone, both sandboxes created.
- **Mode binding**: Create sandbox in `remote` mode, change config to `direct`, create another → both coexist.
- **Self-healing**: Corrupt bare clone, `create` → re-clone works.
- **Cross-module wiring**: `remote_mode_operations_use_bare_clone_not_host` validates that the VcsStore + GitScm pipeline produces the right results — branch in bare clone, not host; archive reads from host via `host_repo_path`; `litterbox` remote is installed; delete cleans up bare clone; remote persists.

### 5.3 Verification
- **AC-1 to AC-14**: All acceptance criteria verified by unit + integration tests.
- **Direct mode unchanged**: All existing tests pass (no regression).
- **Remote mode**: All new tests pass.

---

## 6. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Bare clone corrupt | Self-healing: destroy + re-clone on error |
| Concurrent clones race | Mutex in `ThreadSafeScm` serializes clone creation |
| Host repo changes | Bare clone is independent; re-clone if stale |
| User rebases | Bare clone is independent; fetch/merge works |
| `litterbox` remote conflicts | Check URL before adding; error if mismatch |
| Metadata file stale lock from crashed process | `flock` auto-releases when the holding process dies. No stale lock problem — advisory locks are kernel-managed. |
| **Archive from bare clone** | `make_archive("HEAD")` on bare clone reads the initial shallow clone HEAD, not the user's working tree. Mitigation: GitScm stores `host_repo_path` and opens the host repo temporarily for archive operations. |

---

## 7. Rollout Strategy

1. **Phase 1**: Implement `VcsStore` module (path resolution, bare clone creation).
2. **Phase 2**: Extend `GitScm` with mode field, implement remote mode paths.
3. **Phase 3**: Extend `SandboxMetadata` with new fields, wire configuration.
4. **Phase 4**: Integration tests, self-healing, error recovery.
5. **Final**: Verify all acceptance criteria, direct mode unchanged.

---

## 8. Alignment with Spec

| Spec Requirement | Plan Component |
|------------------|----------------|
| FR-1: Direct mode | GitScm.mode = Direct, existing behavior |
| FR-2: Remote mode bare clone | VcsStore::clone_bare(), VcsStore::resolve_path() |
| FR-3: litterbox remote | VcsStore::install_remote() |
| FR-4: Create branch | GitScm.create_branch() (same in both modes) |
| FR-5: commit_snapshot | GitScm.commit_snapshot() operates on bare clone (remote) |
| FR-6: delete | GitScm.delete_branch() + VcsStore::destroy_bare() (last sandbox only); remote persists |
| FR-7: Self-healing | VcsStore::self_heal() |
| FR-8: Direct mode unchanged | GitScm.mode = Direct, existing behavior |
| FR-9: git2 library | git2::Repository::open() for bare clone |
| FR-10: snapshot-mode default | Config snapshot_mode defaults to Remote |
| FR-11: Metadata persistence | MetadataStore::store() in create flow |
| NFR-1 to NFR-8 | All addressed in component design |
| NFR-9: flock locking | libc::flock-based exclusive lock on metadata file |
| NFR-10: Legacy fallback | load_metadata returns None → Direct mode |
| AC-1 to AC-14 | All verified in testing strategy |

The plan is fully aligned with the spec. Every requirement maps to a component, every acceptance criterion has a verification path.
