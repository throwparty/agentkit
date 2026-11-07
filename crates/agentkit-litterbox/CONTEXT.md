# Litterbox Context

Normative terminology for the Litterbox component. It covers the sandbox model the provider contract introduced; `adrs/2026-11-03-sandbox-provider-contract/` is the decision record and the crate is the implementation.

## Sandboxes and mechanisms

- **Sandbox** — a name, a Git branch and a unit of confined compute, in one project. The name is the identity: one name denotes one sandbox in one backend. There is no handle, id or token beside it.
- **Backend** — an implementation of the contract: a sandboxing mechanism such as Docker, bubblewrap or the macOS Seatbelt sandbox. A backend is a mechanism, not a platform. It states the primitives it requires and is gated by them, so one mechanism may serve more than one host and one host may offer more than one mechanism.
- **Registry** — the compiled-in set of backends and the only place one is selected or constructed. It returns a **Selected** value: the contract object plus the facet objects the mechanism implements.
- **Orchestrator** — the module that pairs a branch with compute. It creates the branch, prepares the workspace, asks the registry for the backend, persists the record and removes the branch when create fails; delete runs compute first and branch second. A backend neither creates nor deletes a branch.

## The contract

- **Contract** — the `Backend` trait: create, enumerate, execute, upload, download, inspect, delete, report readiness and capabilities, and report the provisioning step kinds it can apply. It names no container, image, namespace or transport.
- **Facet** — a group of optional operations reached through a supertrait of the contract: lifecycle (pause and resume), limits, forwarding. A mechanism implements the facets it can, and code that requires one does not compile against a backend that lacks it.
- **Capability** — a backend's report that an operation is available, degraded or unavailable, with a reason and a remedy.
- **Readiness** — the same three states applied to the primitives a mechanism needs on this host, checked before a sandbox is created and before a capability is used.
- **Failure class** — not found, conflict, unsupported, denied, unavailable, misconfigured or failed. The CLI exit code and the MCP error class both derive from it, and the shared numbering lives in `agentkit-common`.
- **Scope** — what an enumeration covers: one repository, or the whole machine.

## Provisioning

- **Provisioning step** — a project-declared unit of preparation with a stable identity, a typed kind and that kind's settings. Identities start with a lowercase letter and use lowercase letters, digits, hyphens and underscores; a step runs at most once in one sandbox.
- **Step kind** — the typed behaviour a step selects. This release defines `command`, which runs a shell command once and confined, with the workspace as its working directory. Kinds are compiled in; an unknown kind fails configuration loading.
- **Sequence** — the ordered list of step identities under `[backends.<name>].provision`. Order is execution order, with no implicit ordering by kind; a backend runs only its own sequence, and the project declares compatibility in `[project] backends` without selecting a backend.
- **Provisioning failure** — a structured refusal or failure naming the class, backend, step identity, kind, reason and remedy. A refused command is denied; an unsupported kind is unsupported; a command that exits non-zero is failed with its exit code and bounded output.
- **Step provenance** — the identity and kind of each step that ran, and the variables it changed by name and kind. It never carries a value.
- **Host step** — a step kind whose work happens on the host during create, before the sandbox exists: it produces effects rather than running in the sandbox. The devShell step is one; a command step is not.
- **Effect** — what a host step contributes to a sandbox, in the vocabulary the contract owns: environment variables, read-only path grants and written files. A backend applies the effects it can honour and refuses the rest, naming the effect and the remedy; the contract names no devShell, flake or store.
- **Read-only grant** — a host path the sandbox may read and not write. A devShell's store closure becomes one grant per store path.
- **Written file** — a file placed inside the sandbox at create, read-only, at a path the backend reports.
- **Store view** — the set of store paths a sandbox can see: a devShell's closure, or the whole store. It is a machine setting, because it trades reachability against the chance that a build reaches a path the derivation never declared; the closure is the default.
- **Differential environment** — the environment taken from a devShell by difference: every exported variable from `print-dev-env`, plus the post-hook value of every variable the shell hook changed, computed under a sanitised caller. Neither `print-dev-env` nor `nix develop` alone is this; the difference is ours to define.

## What a sandbox reports

- **Status** — active, paused, missing, unknown, or error with a reason.
- **Egress posture** — denied, direct, or proxied. Never a boolean, and never inferred from silence: a sandbox on the host network reports direct.
- **Environment provenance** — where a command's environment came from and what the backend changed, by variable name and kind of change. It never carries a value.
- **Workspace** — the path the sandbox sees its work at, fixed by the backend and reported, not assumed.
- **Forward** — a port the project expects, with the host port, the bind address and the environment variable that carries it.

## Records and configuration

- **Record** — the persisted `StoredSandbox`: the contract record (name, backend, project, backend-owned payload) plus the branch and the SCM mode. Persistence keeps SCM vocabulary out of the contract.
- **Legacy record** — a record written before this model. It is reported as unknown and never adopted; no record is synthesised from a name.
- **User configuration** — the person's choice of backend, that backend's settings, setup consent and where forwarded ports bind, in `~/.config/agentkit/litterbox/config.toml`. The only place a mechanism is chosen.
- **Local override** — the untracked `.agentkit/litterbox/config.local.toml`, carrying project-owned replacements only; machine settings and consent are rejected there.
- **Project configuration** — what the project needs: its provisioning definitions, the backends its provisioning supports, expected ports and snapshot mode. It cannot select a backend or dictate confinement.

## Upstream vocabulary

This component uses terms from technologies it does not define. The vocabulary
is a resource of the capability prompt for that technology under
`.agentkit/tackle/prompts/`, linked from the root `CONTEXT.md`; read the
glossary for the word, and the prompt for how the tool behaves.

- [Nix](../../.agentkit/tackle/prompts/nix/glossary.md) — store paths, closures,
  flakes, devShells, evaluation and realisation. Working knowledge: the
  [`nix`](../../.agentkit/tackle/prompts/nix/PROMPT.md) prompt.

## Language

- Generic surfaces — command help, output, errors about sandboxes, MCP schemas and responses — use the words above.
- A mechanism's own vocabulary stays in its own configuration section, messages and module: Docker is described in Docker's terms, and no other backend's terms appear on a generic surface.
