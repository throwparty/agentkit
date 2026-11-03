# Sandbox Provider Contract

**Status:** draft **Created:** 2026-11-03 **Author:** adrian

Litterbox confines agent commands in Docker containers. The trait that abstracts compute is not neutral: `SandboxProvider` (crates/agentkit-litterbox/src/sandbox/mod.rs) names containers, taking a `container_id` and returning a Docker-shaped `ContainerInspection`, and `SandboxMetadata` persists a `container_id` field that metadata_store writes to TOML. The CLI and the MCP server each build `DockerSandboxProvider` independently (main.rs:427, mcp.rs:561), and two bulk paths (main.rs:252 for `pause --all-envs`, main.rs:276 for `pause --all-repos`) call `DockerCompute` directly and bypass the provider. Three spikes established that the kernel's own facilities confine the same workload without a container runtime: bubblewrap reached through Linux user namespaces (adrs/2026-10-31-linux-bwrap-sandbox, adrs/2026-10-31-linux-bwrap-lifecycle) and the `sandbox_init` entry point with a Seatbelt profile on macOS (adrs/2026-10-31-darwin-app-sandbox). They also established that mechanisms differ in kind rather than degree. A bubblewrap sandbox is held open in pid, net, mount and user namespaces and joined with `setns`, so pause, resume and resource limits have a sandbox-level meaning through a cgroup scope; without cgroup delegation, pause falls back to a stop-and-rescan fixpoint that bounds nothing. A Seatbelt profile is applied per process, so there is no sandbox-level state and no freeze or limits, every command runs on the host's pids and network stack, and pause exists only over a process group, which a `setsid` child escapes. Egress needs a proxy in both cases, and the two do not share a transport: a network namespace has no route to the host, so a host-side proxy is not reachable over the network stack at all (linux-bwrap-sandbox/findings.md), while the macOS profile grants only the `localhost` and `*` host tokens (darwin-app-sandbox/findings.md). A shell without any of these mechanisms is also a legitimate choice. A backend is a mechanism, not an operating system: bubblewrap needs Linux namespaces and a writable `/proc`, which a Windows host can provide through WSL, so what a backend requires is a primitive, not the name of the host. Several backends may therefore run on one host, each with its own requirements, its own vocabulary and its own configuration, and the choice between them belongs to the person whose machine runs the code rather than to the project being built. Litterbox needs a contract that says only what the CLI and the MCP server must be able to do to any backend, nothing about how a backend does it, and enough structure for a caller to know which optional operations a backend offers before it relies on one.

## Problem

Litterbox's provider abstraction is Docker's vocabulary wearing a generic name. It assumes one backend per platform that can speak container, so a mechanism that is not a container either implements methods it cannot honour or forks the abstraction, and the CLI and MCP surfaces cannot express a capability that only some mechanisms have or a configuration that belongs to one of them.

## Goals

- Define a backend-neutral contract that Docker, bubblewrap, Seatbelt and future mechanisms all implement
- Model optional operations as facets a mechanism either provides or does not, so a backend is defined by what it can do rather than by the host it runs on
- Report an unavailable or degraded capability before it is used, with a reason and a remedy, and never run a command unconfined
- Give the environment, the workspace and egress an explicit place in the contract, so capture and proxying plug in without amending it
- Let the person running the code choose and configure the backend, and deny that choice to the project being built
- Land the existing Docker backend on the contract with no behaviour change, so a second mechanism has a worked example
- Prove the contract with a conformance suite a new backend must pass
- Enforce the same calling conventions as Tackle: permitted module dependencies declared in arch-lint.toml and checked in continuous integration

## Non-goals

- Implementing the bubblewrap or Seatbelt backend; this contract is the gate they consume, and their conformance is their own acceptance
- The devShell capture procedure, its sanitised baseline and its allowlist (nix-devshell-env-capture)
- The egress proxy: its transport, its policy, its configuration and its lifecycle (egress-proxy)
- Implementing port forwarding for a backend that has none; the capability is reported here, only Docker implements it
- Normalising backend configuration, or standardising backend-specific settings under common names
- Changing snapshot, branch or workspace materialisation semantics
- A new mechanism for a host no mechanism serves, including a native Windows one; a Windows host that offers WSL is served by whichever backend its Linux environment provides
- Virtualisation, OCI images or a container runtime under a backend that does not need them
- Renaming MCP tools that agents already call
- Migrating sandbox metadata written by a previous release
- Performance work on sandbox creation or command execution

## Functional Requirements

### FR-001: Core: Backend-Neutral Contract

The contract carries only operations the CLI and the MCP server need from any backend: create a sandbox, enumerate sandboxes in a stated scope, execute a command, upload a path, download a path, inspect, delete, and report readiness. It names no container, no image, no daemon, no namespace and no mechanism, and it is object-safe, so a caller holds one value whatever the backend the user selected.

**Slug:** `backend-neutral-contract`

### FR-002: Core: Sandbox Identity

A sandbox is identified by its name within a project, and no other identity is introduced. Persisted metadata records the backend that hosts it, so a fresh process or session resolves a name to an existing sandbox with no in-memory state. One name denotes one sandbox in one backend; a backend's own identifier stays private to that backend and never becomes the identity a caller uses. A name with no record is not found, and no record is ever synthesised from a name.

**Slug:** `sandbox-identity`

### FR-003: Core: Backends Are Mechanisms

A backend is a sandboxing mechanism with its own requirements, not an operating system. It states the primitives it needs, such as user namespaces, a writable `/proc`, a code-signing exemption or a daemon socket, and readiness checks those primitives rather than a host name. Several backends may serve one host, a host may have none, and one backend may be built for more than one host where they offer the same primitives. The contract never assumes one backend per platform and never describes a backend in terms of the platform it runs on.

**Slug:** `backends-are-mechanisms`

### FR-004: Core: Facet Families

Optional operations are grouped into facets, each named for the operations it provides rather than for the platform or mechanism that provides them: a lifecycle facet for pause and resume, a limits facet for resource bounds, a forwarding facet for published ports. A backend implements the facets its mechanism can support, so two backends on one host may implement different sets. Code that requires a facet is compiled against a backend that implements it and fails to build otherwise, while the object-safe path in FR-001 reports the same absence at runtime as unsupported, for a CLI that drives whichever backend the user selected.

**Slug:** `facet-families`

### FR-005: Core: Capability Report

Each backend reports which facets and operations it provides, in its own vocabulary. Each entry carries the operation, a state of available, degraded or unavailable, a reason and a remedy. An operation reached through the object-safe path and not provided by the backend fails as unsupported with that reason; it never no-ops, returns an empty answer, or pretends to have run.

**Slug:** `capability-report`

### FR-006: Core: Readiness

A backend reports whether it can run on this host before a sandbox is created, and before an optional operation is used. The report is structured, not prose, and a reason names the primitive that is missing or degraded, its version floor where it has one, and a remedy. Degradation is reported to the caller and never silently substituted; where the degraded operation bounds the workload, the default is to refuse and proceed only on an explicit user configuration decision recorded in the report.

**Slug:** `backend-readiness`

### FR-007: Core: Single Construction Point

Exactly one place selects and constructs the backend from configuration. The CLI and the MCP server obtain it there, and no command reaches a backend's internals to perform an operation the contract does not expose.

**Slug:** `single-construction-point`

### FR-008: Core: Environment Provenance

The environment a command receives is a backend-owned value carrying its provenance as well as its contents: where it came from and what the backend changed. Provenance reports variable names, origins and the kind of change, never values. A backend that must repoint a variable records that; a backend that need not, does not. Callers supply a source of environment, never a finished environment.

**Slug:** `environment-provenance`

### FR-009: Core: Workspace Mapping

The workspace is presented to the sandbox at a path the backend fixes, and the contract reports it. When the sandbox sees the workspace elsewhere than its host path, only references to the workspace move; system paths pass through unmodified. An environment value never widens the view: a host path outside the backend's baseline and the workspace is reported, never newly granted, and mapping applies only to paths already inside granted roots.

**Slug:** `workspace-path-mapping`

### FR-010: Core: Workspace Transfer

Upload and download are defined by the contract rather than by the first backend: both are rooted at the reported workspace path, both are recursive, upload merges into the destination rather than replacing it, and neither can address a path outside the workspace. What a backend cannot transfer is reported as unsupported rather than silently skipped.

**Slug:** `workspace-transfer`

### FR-011: Core: Egress Posture

A sandbox's egress is reported by its backend as one of denied, direct to the host network, or proxied. The posture is never inferred from silence: a backend whose sandbox shares the host network reports direct, so a caller cannot mistake an unconfined sandbox for a confined one. How a proxy is reached, and what it allows, belongs to the egress proxy; this contract only records that egress is proxied and leaves the transport to the backend's own configuration.

**Slug:** `egress-posture`

### FR-012: Core: Neutral Errors

Failures are classified by meaning rather than by wire format: not found, conflict, unsupported, denied and failed. The classification is part of the contract, so the CLI exit code and the MCP error mapping are written once and not per backend. Setup that cannot complete is failed, not unsupported; an operation a backend does not provide is unsupported, not failed.

**Slug:** `neutral-errors`

### FR-013: Core: Port Forwarding

Port forwarding is an optional operation with three separate decisions: which ports a project wants forwarded, where they bind on the host, and who may ask for one. The project may declare that it expects a forwarded port. The host bind interface and the exposure are user configuration alone, default to loopback, and may not be requested through a tool, a command argument or project configuration. A forwarded port is reported with its bind address, and a backend without the forwarding facet fails the ports query as unsupported.

**Slug:** `port-forwarding`

### FR-014: Core: Sandbox Status

The contract defines the status vocabulary and its derivation, so every backend reports the same terms: active, paused, missing and error, with a reason where there is one. A backend without the lifecycle facet reports pause as unsupported rather than never producing the state by accident. Enumerating while a backend is unavailable reports each sandbox as unknown with the reason, rather than omitting it.

**Slug:** `sandbox-status`

### FR-015: Core: Create Is Atomic

Create either returns a sandbox or leaves nothing behind: the backend leaves no compute, no workspace and no scope, and the orchestrator removes the branch it created. A name already in use is a conflict rather than a second sandbox. A setup command runs confined, with the same environment and workspace path a later command receives, and a non-zero exit aborts the create and reports the setup failure.

**Slug:** `create-atomicity`

### FR-016: CLI: Generic Surface

CLI commands, help, output and errors that are not specific to one backend speak of sandboxes, backends and capabilities. Docker terminology stays in the Docker backend, in its configuration section and in messages that concern Docker's own behaviour; no other backend's terms appear on a generic surface either. A sandbox's status comes from its backend rather than from a container state enum.

**Slug:** `cli-surface`

### FR-017: CLI: Backend Introspection

A user-facing command reports the backends available on this host, the selected one, and for each its readiness and capabilities with reasons and remedies, so a user can choose a mechanism and diagnose one that will not run. This adds a new user-facing surface, so its shape, naming and output are to be settled in the plan rather than assumed here. It is not exposed over MCP.

**Slug:** `backend-introspection`

### FR-018: CLI: Bulk Operations Through The Contract

Bulk operations such as pausing every sandbox in a repository or every sandbox on the machine go through the contract, including the enumeration that drives them. No bulk path holds a backend directly, and a sandbox that disappears during a bulk operation is treated as already gone.

**Slug:** `bulk-operations-via-contract`

### FR-019: MCP: Generic Surface

MCP tool schemas and responses carry the sandbox name, its status, its backend, its egress posture and environment provenance by variable name, and no mechanism-specific vocabulary. No tool argument configures a backend; backend configuration is not an agent's to change. Tool names and argument names that agents already call are unchanged, and no response carries an environment value.

**Slug:** `mcp-surface`

### FR-020: Persist: Sandbox Record

The persisted record for a sandbox names the backend that hosts it and whatever that backend needs to address it. Records written by a previous release, which carry a container id and no backend, are not migrated and not adopted: they are reported as unknown records, and no sandbox is ever inferred from one.

**Slug:** `sandbox-record`

### FR-021: Config: Backend Selection

The backend in use is chosen by the person running the code, through user configuration or an explicit per-invocation selection, and never by project configuration, because the sandbox runs on their machine. Project configuration may declare what the project needs of a sandbox but cannot select a backend or set confinement. Selecting a backend that cannot run on this host fails at startup with its readiness reason and remedy, and never falls back to another backend.

**Slug:** `backend-selection`

### FR-022: Config: Per-Backend Schema

Each backend owns a configuration section and defines its own keys, in its own vocabulary. Nothing is standardised across backends beyond what is needed to select one and to address a sandbox, because a shared name for a mechanism-specific setting is lossy. A key a backend does not define is rejected by that backend rather than ignored, and a backend's section is absent when that backend is not selected.

**Slug:** `per-backend-config`

### FR-023: Docker: Contract Conformance

The Docker backend is re-expressed on the contract with its behaviour unchanged: same creation, workspace materialisation, setup command, execution, pause, resume, delete and forwarded-port behaviour, and the same test assertions, now reached through the shared types. It keeps Docker's own terminology in its configuration and its messages, and it is the first implementation the conformance suite is written against.

**Slug:** `docker-conformance`

### FR-024: Arch: Module Boundaries

The contract, each backend, the CLI, the MCP server and persistence are separate modules with declared dependencies: a backend may depend on the contract and the shared error and slug kernel, and on nothing else in Litterbox, and no backend may depend on another; the orchestration that pairs a branch with compute depends on the contract, the registry, SCM and persistence, and is the only module that performs that pairing; the CLI and the MCP server reach a backend through the orchestrator and the construction point rather than through its module; persistence depends on the contract's record types only. A rule broken fails the build.

**Slug:** `module-boundaries`

### FR-025: Orchestration: Branch And Compute Pairing

A sandbox is a branch paired with compute, and the pairing lives in one orchestrator rather than in a backend. The orchestrator creates the branch, prepares the workspace, hands the backend a workspace source, persists the record, and removes the branch when the backend fails to create; delete removes the compute and then the branch, and a half that is already gone is not an error. A backend neither creates nor deletes a branch and is not told that one exists.

**Slug:** `branch-compute-pairing`

## Non-functional Requirements

### NFR-001: Shared: Docker Parity

Docker remains the default backend until the user selects another, and its behaviour does not change. Every existing Docker test assertion holds; only construction and call sites are updated to the shared types. No MCP tool that agents call changes name or shape.

**Slug:** `docker-parity`

### NFR-002: Shared: No New Dependency

The contract adds traits, types and a factory. It adds no crate, no async runtime and no platform library; the bindings a mechanism needs belong to the backend that needs them.

**Slug:** `no-new-dependency`

### NFR-003: Shared: Legible Unavailability

Every unsupported, unavailable or degraded report names the operation, the primitive that failed on this host and a remedy the user can act on, in the backend's own vocabulary. A bare unsupported error is a defect, and every report is structured so a test can assert on its fields.

**Slug:** `legible-unavailability`

### NFR-004: Shared: Primitive Gating

A backend is gated by the primitives it requires, not by the name of the host operating system: where a target cannot provide them, the backend is not built for that target, so its calls are absent rather than failing at runtime, and where the target can provide them but this host does not, readiness says why. A mechanism reached through another environment, such as bubblewrap through WSL on Windows, is a Linux build gated by namespaces and `/proc`, not by the host's name.

**Slug:** `primitive-gating`

### NFR-005: Shared: Test Selection By Module

Each backend's tests live in its own module and are selectable without a runtime skip standing in for a missing host.

**Slug:** `test-selection`

### NFR-006: Shared: Fail Closed

No command runs unconfined. Confinement is established before the workload's first instruction, and its absence, or its loss between create and execute, fails the operation as unavailable. A confinement failure is distinguishable from a command that ran and exited non-zero.

**Slug:** `fail-closed`

### NFR-007: Shared: Documentation From The Contract

CLI and MCP reference documentation are generated from the contract surface, with per-backend documentation generated from each backend's own configuration schema.

**Slug:** `documentation-parity`

### NFR-008: Shared: Calling Conventions Enforced

The module boundaries of FR-024 are declared as scopes and deny rules in arch-lint.toml, in the manner of the ACP boundary already enforced there, and a continuous integration job runs the lint so a violation fails the build rather than being caught in review.

**Slug:** `architecture-lint`

## Acceptance Criteria

### AC-001: Shared: Conformance Suite

A conformance suite drives one generic path through create, enumerate, execute, upload, download, inspect and delete; it passes for the Docker backend and for a test backend that implements the contract and one facet, with no backend-specific branch in the suite.

**Slug:** `conformance-suite`

### AC-002: Shared: Vocabulary Respects Ownership

A search of contract types, generic CLI output, MCP schemas and user-visible errors finds no container, image or daemon term outside the Docker backend, its configuration section and messages about Docker's own behaviour; the Docker section uses Docker's terms, and no generic surface carries another backend's terms either.

**Slug:** `vocabulary-ownership`

### AC-003: Shared: Facets Gate Compilation

A compile-fail test shows a lifecycle operation is not callable through a backend that lacks the lifecycle facet, while the same operation on the object-safe path returns a structured unsupported report naming the backend, the operation, the primitive and the remedy.

**Slug:** `facets-gate-compilation`

### AC-004: Shared: Readiness Refuses Before Running

With a mechanism's primitive absent, creating a sandbox exits non-zero with the required primitive, its version floor where it has one, the reason and the remedy, and no command runs.

**Slug:** `readiness-refuses`

### AC-005: Shared: Degradation Is Reported

On a host without cgroup delegation, limits are reported degraded with the fallback named and what it cannot bound stated, and create refuses unless the user configured otherwise; a backend whose pause is process-group only reports that scope and the escape it does not close.

**Slug:** `degradation-reported`

### AC-006: Persist: Legacy Records Are Not Adopted

A metadata file from a previous release, naming a container id and no backend, produces an unknown record and no sandbox; no compute is addressed from it and no record is synthesised from a name.

**Slug:** `legacy-record-not-adopted`

### AC-007: Persist: Backend Mismatch Refused

A record naming one backend, read where another is selected, is reported with both names and is not operated on by the selected backend.

**Slug:** `backend-mismatch-refused`

### AC-008: Shared: Provenance Is Visible

Inspecting a sandbox reports where its environment came from and which variables the backend changed, by name and kind of change, and carries no value.

**Slug:** `provenance-visible`

### AC-009: Shared: Egress Is Typed

Inspection reports denied, direct or proxied; Docker's bridge reports direct rather than nothing, a namespace-isolated backend reports denied, and no boolean network flag exists in the contract or in any configuration.

**Slug:** `egress-typed`

### AC-010: CLI: Bulk Pause Via The Contract

Pausing every sandbox in a repository and pausing every sandbox on the machine both behave as before, with no direct backend access outside the contract, and a sandbox deleted mid-batch does not abort the batch.

**Slug:** `bulk-pause-via-contract`

### AC-011: Docker: Assertions Unchanged

The full Docker suite passes with unchanged assertions.

**Slug:** `docker-suite-unchanged`

### AC-012: MCP: Tool Names Stable

Every MCP tool name and argument name in use before the change is accepted after it, no tool accepts a backend configuration argument, and responses carry names and statuses rather than container ids.

**Slug:** `mcp-tool-names-stable`

### AC-013: Shared: One Construction Point

The architecture lint finds no backend constructed outside the single construction point and no module reaching a backend's internals.

**Slug:** `one-construction-point`

### AC-014: CLI: Introspection Works

The introspection command lists every backend built for this host, marks the selected one, and reports each one's readiness and facets as structured entries; no MCP tool exposes the same report.

**Slug:** `introspection-works`

### AC-015: Shared: Neutral Errors

Not found, conflict, unsupported, denied and failed map to distinct CLI exit codes and distinct MCP error classes, and the same failure produces the same classification under every backend.

**Slug:** `neutral-errors-accepted`

### AC-016: Shared: Ports Are Honest

A forwarded port reports its loopback bind address; a non-loopback bind appears only when the user configured it; no tool argument or project file can set the bind interface; and a backend without the forwarding facet fails the ports query as unsupported.

**Slug:** `ports-honest`

### AC-017: Shared: Create Leaves Nothing Behind

A create that fails at setup, upload or confinement leaves no compute, workspace or scope, the orchestrator removes the branch it created, and a name already in use is reported as a conflict.

**Slug:** `create-leaves-nothing`

### AC-018: Shared: Boundaries Fail The Build

A change that makes a backend depend on another backend, or the CLI depend on a backend's module, fails the architecture lint, and the continuous integration job that runs it fails with it.

**Slug:** `boundaries-fail-the-build`

### AC-019: Shared: Pairing Is Owned In One Place

A backend can be created from a workspace source and deleted without touching SCM; a failed create leaves no branch; a delete whose compute is already gone still removes the branch; the orchestrator is the only module that creates or deletes a branch.

**Slug:** `pairing-owned-in-one-place`

## Edge Cases

### EC-001: Shared: Mechanism Requirement Unmet

Where the host blocks what a mechanism needs, such as unprivileged user namespaces, whether natively or through an environment hosted on another operating system, readiness reports unsupported with the exact primitive, how to satisfy it, and what happens if it is not satisfied, and nothing falls back to an unconfined run.

**Slug:** `mechanism-requirement-unmet`

### EC-002: Shared: Delegation Absent

Where the host does not delegate what a mechanism needs for limits, limits are reported degraded with what the fallback cannot bound, and the default is to refuse rather than to run unbounded.

**Slug:** `delegation-absent`

### EC-003: Shared: Primitive Missing, Old Or Withdrawn

A mechanism whose primitive is absent, below the required version, or withdrawn by a later SDK or release is reported as unavailable with the floor and the remedy, before any sandbox is created, and the backend never degrades to an unconfined run.

**Slug:** `primitive-missing-or-old`

### EC-004: Shared: Backend Switched

Selecting a different backend while sandboxes exist leaves those sandboxes belonging to the backend that created them; each is reported with its backend, and operating on one through another backend is refused with both names rather than misinterpreted.

**Slug:** `backend-switched`

### EC-005: Shared: Workspace Bound Elsewhere

When the workspace is presented at a path other than the one the environment was captured with, only workspace references are translated, and a translation that would rewrite a system path is a defect.

**Slug:** `workspace-relocated`

### EC-006: Shared: Host Paths In The Environment

An environment from the host that names a path outside the sandbox view is reported, never silently granted and never silently dropped, so the failure is a missing path the user can see rather than a variable that quietly points nowhere.

**Slug:** `host-path-in-environment`

### EC-007: Shared: Sandbox Missing

A name whose record exists but whose sandbox no longer does is reported as missing, and deleting it again succeeds, including the second removal of its branch.

**Slug:** `sandbox-missing`

### EC-008: Shared: Bulk Operation Races A Delete

An enumeration racing a delete treats the vanished sandbox as already gone and finishes the rest, rather than aborting the batch.

**Slug:** `bulk-and-delete-race`

### EC-009: Shared: Unconfined Egress Reported As Direct

A backend whose sandbox shares the host network, as Docker's default bridge does, reports direct egress; a caller that expected confinement sees it in the inspection rather than inferring it from silence.

**Slug:** `direct-egress-reported`

### EC-010: Shared: Project Asks For A Backend

A project configuration that names a backend or sets confinement is ignored for that purpose and reported, so the user's choice stands and the attempt is visible rather than quietly overridden.

**Slug:** `project-asks-for-backend`

### EC-011: Shared: Two Backends On One Host

Where more than one backend can run on this host, each is listed, configured and reported independently; neither is assumed to be the host's answer, and selecting one does not disable the other for sandboxes it already hosts.

**Slug:** `two-backends-one-host`
