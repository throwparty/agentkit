# Tackle Definition Packages

**Status:** draft  **Created:** 2026-10-21  **Author:** adrian

Tackle's definitions — personas, actors, and prompts — are single markdown files discovered from the user and project configuration layers, with project-layer definitions trust-gated per file (adrs/2026-09-17-tackle FR-010, FR-022, AC-010). Two limits have surfaced. A definition cannot carry resources, so a prompt that needs a supporting document or an invokable script has nowhere to put it. And definitions sourced from a shared location have no reproducible delivery mechanism.

The obvious way to deliver resources would be a harness-side virtual filesystem exposing file bytes to the model. But the model's tools are rooted at the repository — the editor workspace, or a sandbox branched from it — so a VFS would only duplicate reachability the repository already provides. Delivering definitions in the repository instead makes resources plain files the model reads with tools it already has. It removes the need for a VFS, a mount resolver, or any coupling to the compute mechanism.

Repository-resident configuration also brings benefits beyond reachability. It is shareable by default, so definitions travel with the project and a team gets the same personas and prompts with no separate distribution step. And it reduces the attack surface to something the agent already needs in order to be productive at all: the repository it is already reading and running.

This amends parts of adrs/2026-09-17-tackle. FR-010 (Reusable Prompts) and FR-022 (Config Discovery) are amended by the directory shape and repository-only sourcing. AC-010 (Trust Gating) is superseded by the trust-boundary reframing.


## Problem

Definitions cannot carry resources, and there is no reproducible way to source them from shared locations. Because the model's file access is rooted at the repository, any resource outside it is unreachable without a harness-side virtual filesystem, which would duplicate the repository's existing reachability.

## Goals

- Definition packages as a directory with an entry point and arbitrary resources for every definition kind
- Resources reachable by the model with no new harness surface via in-repository delivery
- Reproducible sourcing of shared definitions via a manifest and a lock file
- Context variables that expose the package location to the model
- A trust posture honest about the sandbox boundary with no ACP launch-time prompt

## Non-goals

- A harness-side virtual filesystem or resource read tool
- Executing package scripts in the harness
- Materialising resources into compute at runtime
- User-layer definitions that carry resources
- Non-git source formats such as well-known discovery and archives
- Per-definition version constraints beyond a source ref
- A separate binary since the package manager is a tackle subcommand


## Functional Requirements

### FR-001: Definition Packages

A definition is a directory containing an entry-point markdown file plus any number of resource files. The directory name is the definition name and the entry point carries the frontmatter and body exactly as a flat definition does.

**Slug:** `definition-packages`

### FR-002: Entry Point Convention

The entry-point filename is fixed per kind. PROMPT.md for prompts; PERSONA.md for personas; ACTOR.md for actors. A directory whose entry point is missing or misnamed is a load error naming the path.

**Slug:** `entry-point-convention`

### FR-003: Location Discovery

The loader discovers definitions by location in the repository tree at .agentkit/tackle/<kind>/<name>/<ENTRY>.md. Discovery is location-based and independent of any manifest. Flat single-file definitions at .agentkit/tackle/<kind>/<name>.md remain supported for definitions without resources.

**Slug:** `location-discovery`

### FR-004: Context Variables

Every definition body supports harness-provided context variables in the $ namespace. $.name is the definition name and $.dir is the definition directory as a repository-relative path. The $ sigil keeps context variables distinct from declared parameters. Substitution applies to every definition body.

**Slug:** `context-variables`

### FR-005: Resource Listing

The expanded body of a model-facing definition is followed by a listing of its resource paths relative to $.dir so the model knows what it may read. The listing is appended for prompts and personas and not for actors which carry no body.

**Slug:** `resource-listing`

### FR-006: Definitions Manifest

The repository manifest at .agentkit/tackle/definitions.toml declares sources and the definitions taken from each. It is the control point for sourcing only and is not load-time input. Duplicate definition names across sources are a manifest parse error.

**Slug:** `definitions-manifest`

### FR-007: Source Specifications

Each [[source]] declares a required kind field with no default. kind = git carries a [source.git] table of url plus optional ref and path. kind = path carries a [source.path] table with dir pointing at a local directory for authoring and testing. The url field also accepts the PEP 508 git+ URL form.

**Slug:** `source-specifications`

### FR-008: Manifest Delegation

A manifest may declare one or more [[extends]] entries that delegate to a remote definitions.toml resolved by the lock. Delegated manifests are union-merged with the local one. Duplicate names anywhere in the resolved set are a parse error and no override is permitted.

**Slug:** `manifest-delegation`

### FR-009: Lock File

The lock at .agentkit/tackle/definitions.lock records each source resolved to an immutable commit and each definition to a content hash computed over the sorted package files. The lock provides integrity only. It carries no authority and does not gate loading.

**Slug:** `lock-file`

### FR-010: Definitions CLI

The tackle definitions subcommand with the def alias manages sourcing. init scaffolds a manifest; add and remove edit selections; get resolves fetches and vendors; update re-resolves refs and re-vendors; list reports declared and vendored definitions; verify re-hashes the tree against the lock.

**Slug:** `definitions-cli`

### FR-011: Loader Independence

The loader reads no manifest and no lock. Loading is deterministic from the repository tree alone. Integrity drift between the tree and the lock is reported by tackle definitions verify rather than at session start.

**Slug:** `loader-independence`

### FR-012: Trust Boundary

The sandbox is the consent boundary. Context-bearing project configuration such as definition bodies and resources loads without prompt because launching in a repository already admits its content to the sandbox. Bounded Rhai scripts also load because the engine limits are the consent. Unbounded host execution such as mcp_servers and client_tools and endpoints and credential_helper is denied at project scope and enabled only from user configuration outside the repository. There is no launch-time consent prompt.

**Slug:** `trust-boundary`

### FR-013: Built-in Definitions

Built-in definitions remain embedded single-file assets registered at the lowest precedence. A discovered definition of the same name replaces the built-in and the override is surfaced.

**Slug:** `built-in-definitions`

## Non-functional Requirements

### NFR-001: Fail Closed

Sourcing fails closed. A missing or stale lock is an error naming tackle definitions get. An unreachable source or an unreadable manifest fails the operation rather than loading a partial set.

**Slug:** `fail-closed`

### NFR-002: Determinism

The resolved set of sourced definitions is fully determined by the manifest and the lock. get and update are the only operations that touch the network and both write the lock.

**Slug:** `determinism`

### NFR-003: Backward Compatibility

Flat single-file definitions continue to load unchanged. Existing repository trees that predate packages need no migration.

**Slug:** `backward-compatibility`

### NFR-004: Minimal Dependencies

Git sourcing uses the existing git2 dependency. The package manager adds no new runtime dependency beyond TOML and hashing already present.

**Slug:** `minimal-dependencies`

### NFR-005: Security Posture

Fail-closed throughout. Credentials never enter model context or logs. The permission system remains in-core and outside any sandbox. This decision strengthens rather than weakens the posture by denying project-scope host execution outright.

**Slug:** `security-posture`

## Acceptance Criteria

### AC-001: Package Loads

A directory definition at the conventional location loads with its body and resources and appears in the invokable registry under its name.

**Slug:** `package-loads`

### AC-002: Resources Reachable

The model reads a package resource from the repository with its existing file tools using the path exposed by the context variable.

**Slug:** `resources-reachable`

### AC-003: Variables Substitute

{{ $.dir }} and {{ $.name }} substitute in every definition body and a parameter named dir does not collide with the context variable.

**Slug:** `variables-substitute`

### AC-004: Manifest Get

tackle definitions get resolves the manifest sources vendors the selected definitions into the tree and writes the lock.

**Slug:** `manifest-get`

### AC-005: Lock Pinning

tackle definitions update re-resolves refs and rewrites the lock while get reproduces the exact locked revisions.

**Slug:** `lock-pinning`

### AC-006: Verify Drift

Editing any file in a package makes tackle definitions verify report drift for that package and exit non-zero.

**Slug:** `verify-drift`

### AC-007: Collision Error

Two sources selecting the same definition name or a source colliding with a hand-authored location fails manifest parsing with a precise error.

**Slug:** `collision-error`

### AC-008: Trust Posture

With no prompt definitions and bounded scripts load in a repository while project-scope mcp_servers and client_tools and endpoints and credential_helper are ignored.

**Slug:** `trust-posture`

## Edge Cases

### EC-001: Missing Lock

A get or verify without a lock fails closed naming tackle definitions get. The loader still loads the tree because it does not read the lock.

**Slug:** `missing-lock`
### EC-002: Unreachable Source

A source that cannot be fetched fails the operation with the source name and URL and leaves the tree and lock unchanged.

**Slug:** `unreachable-source`
### EC-003: Local Path Source

A kind = path source vendors from a local directory so a definition can be edited and tested before publishing to a git source.

**Slug:** `local-path-source`
### EC-004: Builtin Override

A package that replaces a built-in definition is surfaced as an override at first run exactly as a flat definition is.

**Slug:** `builtin-override`
### EC-005: Flat And Directory Collision

A flat definition and a directory definition with the same name in the same kind directory is a load error naming both paths.

**Slug:** `flat-and-directory-collision`
### EC-006: User-Layer Package

Definitions are repository-only. A definition directory placed in the user configuration directory is not loaded because its resources would be unreachable by the model.

**Slug:** `user-layer-package`
### EC-007: Source Path Traversal

A source definition path or source root that escapes the repository or source root is rejected at manifest parse time.

**Slug:** `source-path-traversal`

