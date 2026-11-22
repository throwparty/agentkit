# Hide VCS Metadata From Sandboxes

**Status:** implemented  **Created:** 2026-11-21  **Author:** Luke Carrier

Litterbox pairs a sandbox with a Git branch and a unit of compute. Today the workspace is always a private copy: `create` calls `vcs.prepare`, materialises the base revision into a `TempDir`, and hands that directory to the backend as `CreateRequest.workspace.source` (`orchestrator/service.rs:140-162`), so the sandbox never sees `.git`. Repair does the same (`service.rs:313-341`).

The map `maps/2026-11-18-litterbox-vcs-history-sync` settles that a bound workspace is also possible. Its `decision` field describes jj as one workspace per sandbox behind a non-colocated bridge with `.jj` masked. T-002 and T-012 answer how: bubblewrap puts a tmpfs over the metadata path, Docker uses an empty volume at it, the contents are hidden, the directory entry remains empty, and a backend that cannot mask refuses `Bound`. Both tickets are `answered`, so masking is decided rather than open.

The decouple ADR left the primitives in place. `BindingSupport::{Copied, Bound, Both}` exists (`vcs/capabilities.rs:10-19`), `Vcs::masked_paths` is on the trait (`vcs/mod.rs:39`), and `GitVcs` reports `Copied` with an empty mask (`vcs/git/scm.rs:388-403`). Its EC-007 settled that `prepare` takes no binding argument and that a combination the backend cannot honour is refused when the configuration is validated, before any sandbox is created, and never falls back to copying.

What is missing is the choice. `StoredSandbox` carries `sandbox`, `branch_name` and `remote` and no binding (`metadata_store.rs:17-25`). `[git]` is project-owned and holds one key, `snapshot-mode` (`config.rs:54-60`), and `reject_project_keys_in_user` refuses it in the user configuration (`config_loader.rs:310-332`). No backend can mask: bubblewrap emits `--tmpfs` for `/tmp` and `/home` only (`backends/bubblewrap/view.rs:318-321`), Docker sets no `Binds` in `HostConfig` (`backends/docker/compute.rs:192-205`), and Seatbelt's renderer emits only allow rules after `(deny default)` (`backends/seatbelt/policy.rs:173-199`).

This ADR adds the choice and makes it safe. `CONTEXT.md` now defines **Workspace setting** and **Workspace binding**.


## Problem

A sandbox that binds a host workspace directory would read and write `.git` or `.jj` through it. Nothing chooses between a private copy and a bound directory, nothing records the choice per sandbox, no compute backend can mask a metadata path, and no configuration path lets a machine say which it wants for one workspace.

## Goals

- A sandbox never reads or writes VCS metadata, whichever binding it was created with.
- The binding is a workspace setting in the untracked local override, keyed per VCS, with no default.
- The binding is recorded per sandbox and honoured by every later operation.
- Bubblewrap and Docker can mask a bound workspace.
- A backend that cannot mask refuses `Bound` rather than degrading.
- A missing key is a configuration error, never a silent copy.

## Non-goals

- The VCS selector. `[vcs] tool` and the `[vcs.jj]` section land with the ADR that introduces jj.
- Darwin masking. It depends on this configuration wiring and on a probe of Seatbelt's deny-rule ordering, recorded as evidence beside the map, and lands as a separate change written and tested on a macOS host. The open question is map ticket T-018, and the backend refuses `Bound` until it is answered.
- The state machine, conflict mechanics and history retention.
- Removing `.git` from a copied workspace. `materialise` unpacks an archive, so `.git` is already absent under `Copied`.
- Making the directory entry absent rather than empty.


## Functional Requirements

### FR-001: The binding enum

`sandbox::contract` owns `WorkspaceBinding` with variants `Copied` and `Bound`, beside `StoreView` and `SnapshotSelector`, because persistence and configuration both need it and arch-lint forbids either from importing `vcs` (`arch-lint.toml:104` and `245`). `BindingSupport` moves there from `vcs::capabilities` so the vocabulary has one home, and `vcs` imports both. The enum has no `Default` impl and no serde default, so a configuration section that omits it is a missing value rather than a silent copy.

**Slug:** `binding-enum`

### FR-002: The configuration key

A machine-owned `binding` key in the `[vcs.<tool>]` section of the untracked `.agentkit/litterbox/config.local.toml`, accepting `copied` or `bound`. The section mirrors `[backends]`: `ProjectFile` gains an untyped `vcs` map, `Config` gains `vcs_settings`, the committed project file's table is noticed and ignored, and the user configuration is refused. `[git]` is deleted and `snapshot-mode` moves to `[vcs.git]`, so it becomes a workspace setting rather than project configuration. `CONFIG_KEYS` drops `git` and gains `vcs`, and `reject_project_keys_in_user` gains a `vcs` clause.

**Slug:** `config-key`

### FR-003: No default

`create` refuses with a configuration error naming the key, the accepted values and the file when `binding` is absent. There is no fallback to `Copied`.

**Slug:** `no-default`

### FR-004: The record field

`StoredSandbox` gains `binding`, written at create and read by every later operation, so a configuration change affects only new sandboxes.

**Slug:** `record-field`

### FR-005: Legacy records

A record written before this change has no binding field. It loads as `Current` and is treated as `Copied`, because that is how it was created. It is never rewritten and is reported as a legacy fact.

**Slug:** `legacy-records`

### FR-006: Validation before create

The selected VCS reports the bindings its configuration can honour, and a combination it cannot honour is refused when the configuration is validated, before any sandbox is created. Git reports `Copied`, so `bound` is refused for Git.

**Slug:** `validation`

### FR-007: The mask contract

`masked_paths` names `.git` and `.jj`. Under `Copied` the backend receives a private copy that excludes them, which `materialise` already guarantees because it unpacks an archive. Under `Bound` the compute backend masks each one.

**Slug:** `mask-contract`

### FR-008: Bubblewrap masks

Bubblewrap renders a tmpfs over each masked path inside the bound workspace, so the contents are hidden and the directory entry remains empty.

**Slug:** `bwrap-masking`

### FR-009: Docker masks

Docker mounts an empty volume at each masked path inside the bound workspace. It refuses `Bound` when it cannot guarantee that the daemon shares the host filesystem, because the bind source is interpreted on the daemon's side.

**Slug:** `docker-masking`

### FR-010: A backend that cannot mask refuses

A compute backend reports whether it can mask. One that cannot refuses `Bound` with a structured unsupported failure naming the backend, the binding and the remedy. Seatbelt cannot mask today, because its renderer emits no deny rule, so it refuses `Bound`.

**Slug:** `mask-refusal`

### FR-011: The bound directory

The VCS backend owns one host directory per bound sandbox, derived from the project slug and sandbox name the way `bubblewrap::workspace::sandbox_directory` derives its state directory. The path is not stored in the record, because the backend recomputes it and a stored path goes stale when the data directory moves.

**Slug:** `bound-directory`

### FR-012: Removal and orphans

The contract names both releases: `delete_branch` releases the branch and any host directory the backend owned for that sandbox, and `prune` releases project storage that is no longer needed, including bound directories whose branch is gone. The orchestrator delivers its half: the `prune` call condition widens from `stored.remote` to also cover a record whose binding is `Bound` (`orchestrator/service.rs:478-482`), and a failed create reaches the same `delete_branch` on every path that already removes the branch (`service.rs:145-147`, `172-174`, `191-193`). Git owns no bound directory, so both releases are contractual here and the binding VCS implements them; that work lands with the jj bridge. Until then no bound directory can exist and no sweep has anything to find.

**Slug:** `orphans`

## Non-functional Requirements

### NFR-001: The project cannot set the binding

The committed project file never carries `binding`, and a configuration that tries is refused or ignored with a notice, matching how `store-view` and `forwarding` are handled (`config.rs:399-419`).

**Slug:** `project-cannot-set`

### NFR-002: The record decides

Later operations read the binding from the record, never from the current configuration.

**Slug:** `record-decides`

### NFR-003: No silent fallback

Every refusal names the class, the key or value, the reason and the remedy. No path substitutes `Copied` for a requested `Bound`.

**Slug:** `no-fallback`

### NFR-004: The gates run in CI

`cargo test -p agentkit-litterbox`, `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`, `arch-lint check` and `bash hack/arch-lint-negative.sh` pass.

**Slug:** `gates`

## Acceptance Criteria

### AC-001: No VCS data either way

A sandbox created with `Copied` and one created with `Bound` both expose no VCS metadata: `.git` and `.jj` are absent from the copy, or present as an empty directory entry under a mask, and no read or write reaches the VCS.

**Slug:** `no-vcs-data`

### AC-002: A missing key is refused

`create` without `binding` fails with a configuration error naming the key and the file.

**Slug:** `missing-key-refused`

### AC-003: Legacy records load as Copied

A record file with no binding field loads as `Current` with `Copied`, and its sandbox still lists, deletes and repairs.

**Slug:** `legacy-loads`

### AC-004: A backend that cannot mask refuses Bound

A `Bound` request against a backend that cannot mask fails with an unsupported refusal naming the backend and the remedy, and no sandbox is created.

**Slug:** `cannot-mask-refuses`

### AC-005: The gates pass

`cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` pass, and `arch-lint check` and the negative fixture are unchanged.

**Slug:** `gates-pass`

## Edge Cases

### EC-001: The empty mountpoint

Masking hides the contents and leaves the directory entry. An empty `.git` is not an absent `.git`, and this ADR does not claim absence. T-002 records that absence needs an overlay or whiteout that is unproved on both mechanisms.

**Slug:** `empty-mountpoint`
### EC-002: The path is derived, not stored

The bound directory is a function of the project slug and sandbox name. A record does not carry it, so moving the data directory does not orphan a sandbox.

**Slug:** `derived-path`
### EC-003: A failed create leaves no directory

Create removes the bound directory on every path that already removes the branch, including a provisioning failure and a record-store failure.

**Slug:** `failed-create`
### EC-004: A corrupt record leaves an orphan

A record that cannot be read cannot be addressed, so a forced delete removes the record and the compute state but nothing knows a bound directory existed. `prune` is the only path that finds it.

**Slug:** `corrupt-orphan`
### EC-005: Docker's daemon is local

Docker connects over a local unix socket and refuses `tcp://` (`backends/docker/compute.rs:630-667`), so a bind source is read on the daemon's side. A daemon whose filesystem is not the host's cannot honour `Bound` and must refuse it.

**Slug:** `docker-local`
### EC-006: `[git]` fails as an unknown field

Removing the `git` field from `ProjectFile` makes a configuration that still sets `[git]` fail to parse, because `ProjectFile` derives `deny_unknown_fields`. No migration code is written.

**Slug:** `old-git-key`

