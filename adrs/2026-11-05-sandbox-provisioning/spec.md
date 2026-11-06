# Sandbox Provisioning

**Status:** implemented  **Created:** 2026-11-05  **Author:** adrian

Litterbox creates a sandbox from a project and a machine-selected backend. Today the only preparation a project can declare is `setup-command`, a single string under `[project]`, and the environment is either the backend's default (`EnvironmentSource::Mechanism`) or a host-captured set of variables (`EnvironmentSource::Captured`); the orchestrator hardcodes `Mechanism` (crates/agentkit-litterbox/src/orchestrator/service.rs:154). The provider contract deliberately keeps backends mechanism-neutral and denies the project the choice of backend (adrs/2026-11-03-sandbox-provider-contract, FR-021), yet a project must be able to ship a configuration that works for every engineer who selects a supported backend. The Nix devShell work adds a second kind of preparation that only some backends can serve, and it needs an ordered relationship with the setup command: the devShell environment must exist before the command runs. One string and one environment enum cannot express that. This ADR replaces both with an ordered list of identity-named, typed provisioning steps, selected per backend by the project and resolved for the backend the machine selected. It is the gate for `nix-devshell-env-capture`, which adds the `nix` kind, and for the native backends that consume it. The model follows Vagrant's provisioners: each provisioner has a name, a type and options, and a machine runs them in the order it declares them (https://developer.hashicorp.com/vagrant/docs/provisioning/basic_usage). The existing wording is FR-001 to FR-026 of the provider contract; the existing project-owned key is `[project] setup-command` in crates/agentkit-litterbox/src/config.rs:48.


## Problem

Today the provider contract carries exactly one environment and one setup command: `CreateRequest.environment` is either the backend's default or one captured set of variables, and `CreateRequest.setup_command` is a single string. There is no list, no identity and no order. A project cannot declare a sequence of preparation steps, cannot say which steps apply to which backend, and a backend cannot report which preparation it can perform. The result is that a project cannot ship a configuration that works across the backends its engineers select, `setup-command` is a special case of a general need, and the Nix devShell would have to be bolted on as a second special case rather than as one kind among several.

## Goals

- Replace CreateRequest.environment and CreateRequest.setup_command with an ordered, typed provisioning sequence.
- Let a project define steps once, by identity, and declare the ordered sequence for each backend it supports.
- Keep backend selection machine-owned; the project declares compatibility, not selection.
- Let a backend report which step kinds it can apply, and fail a listed step it cannot with a reason and remedy.
- Preserve setup-command behaviour as a command step under the existing consent.
- Add a doctor/preflight report that shows the same readiness at any time, with no mutation.
- Report provenance per step without values.

## Non-goals

- The nix kind's capture, fields, host checks and sandbox behaviour (nix-devshell-env-capture).
- Automatic mechanism selection (automatic-backend-selection); the selected backend remains the machine's choice.
- Changing setup consent semantics.
- Changing ports, SCM, snapshots or workspace materialisation.
- A runtime plugin system for step kinds; kinds are compiled in and added by the ADR that owns them.
- Migrating existing configuration files; setup-command is rejected with the replacement named.
- Exposing the doctor over MCP; the doctor is a CLI surface, and create carries the same structured failures to MCP.
- Running backend-specific setup outside the sandbox; a command step runs confined.


## Functional Requirements

### FR-001: Core: Provisioning Step

A provisioning step has an identity unique within the project, a kind, and settings the kind defines. The identity is stable, starts with a lowercase ASCII letter, and continues with lowercase letters, digits, hyphens or underscores; it is how a backend's sequence references the step, how a failure names it and how inspection reports it. A step runs at most once in one sandbox. A project may declare at most 32 steps.

**Slug:** `provisioning-step`

### FR-002: Core: Step Kinds

Step kinds are compiled in and typed. This ADR defines `command`; `nix-devshell-env-capture` adds `nix`. A kind defines its own settings and its own readiness. An unknown kind is rejected when configuration loads, naming the kind and the known kinds; it is never ignored or treated as a command.

**Slug:** `step-kinds`

### FR-003: Core: Backend Sequence

Each backend carries an ordered `provision` list of step identities, under `[backends.<name>].provision`. The list order is the execution order; there is no implicit ordering by kind. A repeated identity in one list is an error. A backend with no `provision` key, or an empty list, runs no steps. A list naming an identity the project does not define is an error naming the identity and the backend. One sequence may list at most 32 identities.

**Slug:** `backend-sequence`

### FR-004: Core: Project Compatibility, Machine Selection

The project declares the backends its provisioning supports in `[project] backends`, a list of backend names. The list is required when `[[provision]]` defines at least one step; with no steps it is optional, and absent means no restriction. The local override may replace it. The project may not select a backend: selection remains in the machine-owned local override or the per-invocation flag, as FR-021 of the provider contract requires. A selected backend not in the list fails naming the backend and the list; the local override may add it. Any key under `[backends.<name>]` other than `provision` in the committed project file remains machine-owned and is ignored with the existing notice.

**Slug:** `project-compatibility`

### FR-005: Core: Resolution

Exactly one place resolves the selected backend's identity list against the definitions into an ordered list of typed steps, validates identities and kind support, and returns it for `CreateRequest`. The backend receives only its own resolved steps; it never sees the supported set, another backend's name, or a step the project did not list for it.

**Slug:** `provision-resolution`

### FR-006: Core: Kind Support

A backend reports the step kinds it can apply, as a typed list used by resolution and as capability entries for the generic surfaces. A list naming a kind the backend does not report fails resolution with the backend, the step identity, the kind, the reason and the remedy; the backend is never asked to apply it. A kind the backend reports as degraded follows the contract's existing degradation policy.

**Slug:** `kind-support`

### FR-007: Core: Contract

`CreateRequest.environment` and `CreateRequest.setup_command` are removed and replaced by `provision: Vec<ProvisionStep>`. An empty list is the backend's default environment. A step is a typed value; `ProvisionStep::Command { identity, run }` runs once, confined, with the environment produced by the steps before it. A command step's environment changes do not persist: only an environment-producing kind changes what later steps and commands see. `EnvironmentSource` is removed; `Environment` and `Provenance` remain for inspection.

**Slug:** `provision-contract`

### FR-008: Command: Consent

A `command` step runs project code, so it is consent-gated exactly as `setup-command` is today: the project's namespace is in `allowed-projects`, or `allow-project-setup` permits every project. A refusal names the step identity and both settings. A non-zero exit fails create with the step identity, the exit code and the output.

**Slug:** `command-consent`

### FR-009: Core: Atomicity

Create is atomic across the sequence: a step that fails leaves no compute, workspace or scope, and the orchestrator removes the branch it created, exactly as a failed create does today. No partially provisioned sandbox is returned.

**Slug:** `provision-atomic`

### FR-010: CLI: Doctor

A user-facing preflight command reports, without creating or mutating anything: the selected backend and its readiness; the project's steps in the selected backend's order, each with state, reason and remedy; and the project/backend compatibility. Its name, flags and output are settled in the plan, like the existing backend introspection. It is not exposed over MCP. Create returns the same structured failures, and the MCP error payload carries them so an agent can tell the user what is missing.

**Slug:** `doctor`

### FR-011: Core: Provenance

Inspection reports the steps that ran, by identity and kind, and for each the variables it changed, by name and change kind, never by value. A step that set, repointed or removed a variable records it; a step that changed nothing records nothing. The whole-environment origin remains for the case where no step ran.

**Slug:** `step-provenance`

### FR-012: Config: Ownership

`[project] backends` and `[[provision]]` are project-owned; the local override may replace both. `provision` under `[backends.<name>]` is the one project-owned key there; every other key remains machine-owned and is ignored with a notice when it appears in the committed project file. Configuration loading extracts `provision` from the backend table before the backend deserialises its own section with `deny_unknown_fields`, so no backend sees it. The local override may replace the per-backend lists wholesale, and validation applies to the merged result.

**Slug:** `config-ownership`

### FR-013: Config: Setup Command Rejected

`[project] setup-command` is removed. A configuration that still sets it fails to load with a message naming the replacement: a `command` step with its own identity, listed in the sequence of each backend that should run it. No automatic migration is performed.

**Slug:** `setup-command-migrated`

### FR-014: Core: Failure Classification

A provisioning failure uses the contract's existing classes: an unlisted backend, unknown identity or duplicate identity is `Misconfigured`; a kind the backend does not support, or reports as unavailable, is `Unsupported` with the backend's reason and remedy; a degraded kind the user has not accepted is `Unavailable`; a refused command is `Denied`; a failed command is `Failed` with the step identity, exit code and output. The CLI exit codes and MCP error classes are the existing mapping, not a new one.

**Slug:** `provision-failures`

## Non-functional Requirements

### NFR-001: Shared: No New Dependency

The change adds types, a resolver and configuration fields. It adds no crate, no async runtime and no platform library.

**Slug:** `no-new-dependency`

### NFR-002: Shared: Legible Unavailability

Every unsupported, not-ready or refused step names the step identity, kind, backend, primitive or setting, reason and remedy. A bare failure is a defect, and every report is structured so a test can assert on its fields.

**Slug:** `legible-unavailability`

### NFR-003: Shared: Fail Closed

No listed step is skipped silently. If a step cannot run, create fails; the only way to omit a step is to remove it from the selected backend's list in the project or the local override.

**Slug:** `fail-closed`

### NFR-004: Shared: Resolver Boundary

The resolver is its own module under `src/provision/`, depends on the contract and configuration only, and is reached by the orchestrator. A backend does not depend on it, and the CLI and MCP reach it only through the orchestrator. The dependency is declared in `arch-lint.toml` and checked in CI, as the existing contract boundaries are.

**Slug:** `resolver-boundary`

### NFR-005: Shared: Test Selection

The resolver's tests live in its module and are selectable by package and target. Configuration tests cover the ownership split, the local replacement and the `setup-command` rejection.

**Slug:** `test-selection`

### NFR-006: Shared: Documentation

The configuration reference documents `[[provision]]`, the `command` kind, the per-backend sequence and the `setup-command` replacement. CLI and MCP references regenerate; MCP tool names and arguments do not change.

**Slug:** `documentation`

### NFR-007: Shared: Compatibility

MCP tool names and arguments are unchanged. A project with no `[[provision]]` section behaves as it does today: the backend's default environment and no setup command. The one breaking change is the rejected `setup-command` key, with a message naming the replacement.

**Slug:** `compatibility`

### NFR-008: Shared: Order Is Explicit

The sequence is the only ordering. The resolver never reorders by kind, and a test with two command steps declared in an order that is not alphabetical asserts they run in list order.

**Slug:** `explicit-order`

## Acceptance Criteria

### AC-001: Shared: Ordered Sequence

Two `command` steps in one backend's list run in list order; the second observes a file the first wrote in the workspace.

**Slug:** `ordered-sequence`

### AC-002: Shared: Per-Backend Selection

A project lists `bubblewrap` and `seatbelt` in `[project] backends`, gives both `devshell` and `fetch`, and gives `docker` `fetch` only; each backend's `CreateRequest` carries exactly its list, in its order, and no other backend's steps.

**Slug:** `per-backend-selection`

### AC-003: Shared: Unlisted Backend Refused

With steps defined and a selected backend absent from `[project] backends`, create fails naming the backend and the list, and no workspace, scope or branch remains.

**Slug:** `unlisted-backend-refused`

### AC-004: Shared: Unknown Identity

A backend list naming an undefined identity fails at load or resolution, naming the identity and the backend.

**Slug:** `unknown-identity`

### AC-005: Shared: Unsupported Kind

A backend list naming a kind the backend does not report fails with the backend, step identity, kind, reason and remedy, and the backend is never called.

**Slug:** `unsupported-kind`

### AC-006: Shared: Setup Command Replaced

A configuration with `[project] setup-command` fails to load with a message naming a `command` step and the per-backend list; the equivalent `command` step reproduces the old behaviour, including confined execution and the environment.

**Slug:** `setup-command-replaced`

### AC-007: Shared: Consent Unchanged

A `command` step in an unlisted project is refused naming the identity and both settings; the same project in `allowed-projects` runs it; `allow-project-setup` permits every project.

**Slug:** `consent-unchanged`

### AC-008: Shared: Atomic Failure

A `command` step that exits non-zero leaves no compute, workspace or scope, and the orchestrator removes the branch; the error carries the identity, exit code and output.

**Slug:** `atomic-failure`

### AC-009: Shared: Provenance Per Step

Inspection reports each step by identity and kind and the variables it changed by name and kind, and the rendered report contains no variable value.

**Slug:** `provenance-per-step`

### AC-010: CLI: Doctor Does Not Mutate

The doctor reports the selected backend, its readiness, each step's state and remedy, and the project/backend compatibility; running it creates no sandbox, workspace, branch or store path.

**Slug:** `doctor-no-mutation`

### AC-011: Shared: Matrix Does Not Leak

A test backend records the `CreateRequest` it receives; it contains only the resolved steps for itself and no other backend's name or the supported set.

**Slug:** `matrix-not-leaked`

### AC-012: Config: Local Override

A local override replacing `[project] backends`, `[[provision]]` and a backend's list is what the doctor and create see; the project's definitions are not merged back in.

**Slug:** `local-override`

### AC-013: Shared: Empty Sequence

A project with no `[[provision]]` and no `[project] backends` creates as it does today on any selected backend: the backend's default environment, no setup command, no error.

**Slug:** `empty-sequence`

### AC-014: Shared: Boundaries

The architecture lint declares the resolver scope and denies a backend importing it; a negative fixture fails the build.

**Slug:** `boundaries`

### AC-015: Shared: Unknown Kind

A `kind` the binary does not implement fails to load, naming the kind and the known kinds.

**Slug:** `unknown-kind`

## Edge Cases

### EC-001: Shared: Unused Step

A step defined in `[[provision]]` but absent from every backend's list is reported as a warning at load, not an error; it never runs. A `provision` list for a backend not in `[project] backends` is reported the same way.

**Slug:** `unused-step`
### EC-002: Shared: Duplicate In Sequence

The same identity twice in one backend's list is an error naming the identity and the backend.

**Slug:** `duplicate-in-sequence`
### EC-003: Shared: Same Step, Two Backends

One defined step may appear in several backends' lists; it is one definition and runs once in each selected backend's sequence.

**Slug:** `same-step-two-backends`
### EC-004: Config: Local Empty

A local override that sets an empty `[[provision]]` or an empty backend `provision` list is a deliberate removal, not a fallback to the project's list; the doctor reports the empty sequence. The supported set is replaced separately in `[project] backends`.

**Slug:** `local-empty`
### EC-005: Config: Backend Settings Alongside

A project file that puts a machine-owned key next to `provision` under `[backends.<name>]` is ignored with the existing notice; `provision` is honoured.

**Slug:** `backend-settings-alongside`
### EC-006: Shared: Unbuilt Backend Listed

A project may list a backend the binary does not build; the list is inert. Selecting that backend fails as unavailable under the contract's readiness, not as a provisioning error.

**Slug:** `unbuilt-backend-listed`
### EC-007: Shared: Command Before Environment

A sequence may place a command before the step that provides its environment; this is allowed and the command sees the default environment. The doctor reports the order as written and does not reorder it.

**Slug:** `command-before-environment`
### EC-008: Shared: Denied Step

A refused `command` step is `Denied` and fails create; it is not treated as unsupported, and no later step runs.

**Slug:** `denied-step`
### EC-009: Config: Project File Only

A project that defines steps but no `[project] backends` fails to load, naming the missing list; a project that defines the list but no per-backend `provision` lists runs no steps on its supported backends.

**Slug:** `project-file-only`
### EC-010: Config: Identity Shape

An identity that is empty, does not start with a lowercase letter, or contains a character outside lowercase letters, digits, hyphens and underscores is rejected at load, naming the identity.

**Slug:** `identity-shape`

