# Darwin Seatbelt Provider

**Status:** implemented  **Created:** 2026-11-14  **Author:** adrian

The map at `maps/2026-10-31-nix-devshell-sandbox` chose the `sandbox_init` entry point family for macOS and recorded a prototype at `adrs/2026-10-31-darwin-app-sandbox`. That prototype proved the mechanism confines an unsigned binary and builds a devShell, but it also makes two assumptions this ADR removes: its baseline hardcodes `/nix/store`, and its helper rebuilds the child environment from a seven-name allowlist. Both conflate the mechanism with Nix. That prototype code is the production implementation: this ADR promotes `poc/custom` into the crate rather than rewriting it, keeping its helper and profile renderer. It supersedes the spike's note that its code is discarded with its ADR. The changes are bounded: the hardcoded store baseline, the environment allowlist, and one profile per invocation. T-043 to T-049 answer the questions that separate them. T-043 shows a platform baseline plus a workspace confines a command with no store grant; a Nix binary aborts on its ungranted dylib. T-044 shows the closure cannot be rendered one rule per path: 238 grants cost about 43ms per command, the serialised profile is capped at 65535 bytes, and 1779 grants are rejected. T-045 shows a later process can stop, continue and kill a recorded process group, and that a `setsid` child escapes it while staying confined. T-046 showed the developer directory is a toolchain input rather than a platform constant and that `Contents/Developer` alone is insufficient; T-053 shows a project can supply the installation root and the licence state as read mounts, so this ADR keeps no Xcode knowledge. T-052 settles lifecycle: a resident holder is the group leader, so a background command stays addressable after its shell exits. T-054 and T-055 show every dynamic SBPL value can be a parameter, so the renderer has no escaper. T-047 shows the helper can pass the contributed environment through untouched. T-048 shows readiness needs a probe process as well as a symbol. T-049 shows fork+apply is not something to rely on in a threaded server, so the server never applies a profile; the holder is single-threaded and applies it in each child. This backend follows `adrs/2026-11-07-linux-bwrap-provider`: the contract is mechanism-neutral, and this backend renders contributions rather than knowing a kind. The two backends share the contract surface, the resolved environment with provenance, and an opaque proxy endpoint; everything else differs on purpose, including the unit of execution, the isolation mechanism, the lifecycle primitive and the proxy transport. Assumptions this specification makes, and what would falsify each. First, that the collapse threshold of 64 paths is safe: 64 grants measured 11.7ms and 100 measured 17.0ms against 42.7ms for 238, and the assumption is falsified if a real devShell profile with 64 grants costs materially more because its rule text is larger. Second, that a devShell closure has one common ancestor: T-022's 238 paths all sit under the store root, and the assumption is falsified by a closure spanning two roots, which the renderer refuses rather than widens. Third, that a compiler's writes to the host user temp directory are non-fatal: measured with clang and xcrun, falsified by a toolchain that fails without them. Fourth, that `sandbox_init_with_parameters` remains exported and enforcing on the target macOS: measured on 27.2.1, falsified by a release without the symbol or with a profile the language no longer applies. Fifth, that a sandboxed listener cannot be reached: T-020 left it untested and the profile emits neither bind nor inbound rule, so ports are refused rather than claimed, and a probe that binds and connects would falsify the refusal.


## Problem

Litterbox has a Docker backend and a Linux bubblewrap backend. On macOS neither works: Docker needs a Linux VM, and bubblewrap is Linux-only. The prototype shows the platform sandbox is viable, but no backend implements the contract, so a project that declares a devshell step is refused on macOS and a host without Docker has no mechanism at all. This ADR promotes the prototype, and three of its choices have to change before it ships. Its profile assumes a Nix host, its helper decides the environment, and its closure rendering does not survive the profile limit. Those are bounded edits to code that already confines real workloads. A production backend has to work for a sandbox with no provisioning steps and no Nix, and combine with a devShell as an optional contribution.

## Goals

- Add a Seatbelt backend that implements the existing contract, selected by the person running the code, leaving Docker and bubblewrap behaviour unchanged.
- Confine every command through a helper process that applies a profile and execs; the server never confines itself.
- Render the platform baseline from OS paths only, so the mechanism works on a host with no Nix and no devShell.
- Render contributed mounts as read grants, collapsing a large contribution to an ancestor rather than exceeding the profile limit.
- Pass the contributed environment through unchanged, withholding values that name ungranted paths and rebasing workspace values in the backend.
- Discover the Xcode installation at readiness and grant it only as a toolchain input, never as a baseline constant.
- Define pause, resume and delete through the holder's process group, so a background command stays addressable, and accept that a setsid child escapes it.
- Report readiness for the symbol, the probe, the helper and the workspace, with a reason and a remedy, and refuse rather than degrade.
- Test the renderer, the collapse rule, the environment merge, the record and the lifecycle, and document the configuration.

## Non-goals

- The egress proxy and any destination policy. This ADR gives the sandbox no egress by default and direct egress as an explicit posture; it renders the loopback-port rule the proxy will use but starts no proxy and adds no configuration key for it.
- Resource limits. Seatbelt has no CPU, memory or task quota, so the Limits facet is not implemented.
- Toolchain discovery. The backend does not run `xcode-select` or grant an installation; a project contributes the paths a compiler needs.
- Port forwarding and listeners. The profile emits neither network-bind nor network-inbound.
- Automatic backend selection. The person running the code selects seatbelt.
- The entitlement-based App Sandbox. It needs a signed bundle, and this backend confines unsigned binaries.
- Signing, notarisation, packaging, or the Mac App Store.
- GUI applications, LaunchServices starts and anything that is not a descendant of the helper.
- Closing the setsid escape. macOS has no pid namespace, so the group is the only handle.
- A userland the backend supplies. What a sandbox can run is what its steps contribute.
- Snapshot, command line and MCP work beyond what the contract already does, and the Linux backend.


## Functional Requirements

### FR-001: Registration

The registry gets an entry named seatbelt, with a constructor that reads its own section of the machine-owned configuration. DEFAULT stays docker, so a project or command that selects nothing behaves as it does today. A person selects it with `backend = 'seatbelt'` in the user configuration or `--backend seatbelt`. A project file cannot select it. A project that does not list it in `[project] backends` is refused before create, as it already is for Docker. The module is compiled on macOS only and the registry does not list it elsewhere. One module declares the section's keys, and an unknown key fails as Misconfigured and names the key. A test asserts the entry exists on macOS and is absent elsewhere.

**Slug:** `backend-registration`

### FR-002: Helper: Holder and Command

The helper is the prototype's `sandbox-custom`, promoted into the crate as a second binary, `litterbox-seatbelt-helper`. Its renderer is kept; the arguments it accepts and the source of its environment change. create starts it once per sandbox in `--hold` mode with a socket path. The backend connects and sends the workspace, the read and write grants, the baseline reads, the mach services, the network posture, the tag and the environment over the socket, so no value reaches argv. The holder does not apply a profile to itself, because it needs a host unix socket and must stay able to fork. It renders the profile once, listens on a 0600 unix socket at a recorded path in the user temp directory, because a state path can exceed the socket path limit, and handles each request by forking a child that calls `sandbox_init_with_parameters` with the text and its parameter table, then execs the command. Every dynamic value is a parameter: paths, the tag, mach service names and loopback endpoints. The tag is the sandbox name, validated as `[A-Za-z0-9._-]{1,64}`. It multiplexes concurrent requests over pipes without threads. The backend resolves the helper beside its own executable, refuses a path that is missing, not a regular file, or group or world writable, and records the resolved path. No caller supplies SBPL and no profile text reaches argv. For each command it reports an apply error with the `sandbox_init_with_parameters` message when the child cannot be confined, a policy error with its reason when it refuses the profile, and an exec error when the child cannot exec. It never runs a command unconfined. A failure after the holder starts kills it and removes the state directory. The server never calls `sandbox_init_with_parameters` or `pre_exec`; only the holder's forked child applies a profile. It does not link the server's runtime, logging or configuration.

**Slug:** `helper-binary`

### FR-003: Profile: Platform Baseline

The helper's renderer emits a deny-default profile whose read baseline is OS paths only by default: `/System/Library`, `/System/Applications`, `/Library/Frameworks`, `/usr/bin`, `/usr/lib`, `/usr/share`, `/usr/libexec`, `/bin`, `/sbin`, `/private/etc`, `/private/var/db/dyld`, `/private/var/select`, `/dev`, and the root literal needed by the dynamic linker. A machine may add paths under `baseline-reads` and mach services under `mach-services`. With `baseline = 'none'` the lists replace the defaults, so a machine can opt out and harden. It contains no `/nix` path, no store path and no home directory unless a machine adds one. The profile grants process-exec and process-fork, process-info and signal to the same sandbox, file-read-metadata scoped to the baseline, the grants and their ancestors with every ancestor written as a literal, sysctl-read, writes only to the workspace and `/dev/null`, and mach-lookup over the default list plus additions. The ancestor literals are required: a global metadata rule still lets a denied path be `stat`ed, and narrowing the rule to the grant subtree breaks path resolution with ENOTDIR because the kernel stats each ancestor first. The root literal is mandatory: without it every exec dies with SIGABRT and no output. The backend resolves configured and contributed paths at create and refuses an unresolvable one as Misconfigured. The helper resolves every value again before it becomes a parameter and returns a policy error if one still fails. Every dynamic value is a parameter, so the renderer escapes nothing: `sandbox_init_with_parameters` substitutes it out of band.

**Slug:** `profile-baseline`

### FR-004: Mounts: Read Grants and Collapse

Each accepted contributed path becomes a read grant. When a contribution resolves through a symlink, the path as named and each named ancestor are also granted as literals, so a command can follow the path it was given; the grant itself stays on the resolved target. The workspace is the only writable host tree. Before rendering, the backend validates every contributed path the way the Linux backend does: absolute, resolvable, not a socket, device or FIFO, not the workspace or an ancestor of it, and not a protected root, the metadata directory or the backend's state directory. A contribution whose paths resolve under `/Users` or the current user's home is refused. When a step contributes more than 64 paths, the backend replaces that step's list with the longest common ancestor of its paths; for a devShell closure that is the store root. When the collapsed set is still more than 64 paths, or the common ancestor is `/`, create fails as Misconfigured and names the step and the count. Granting `/` would read the host. The helper renders the flat grant list and refuses above 60 KiB of profile, returning a policy error with the size. An oversized profile can abort libsandbox instead of returning an error, so the backend maps that policy error to Misconfigured.

**Slug:** `mounts-collapse`

### FR-005: Environment: Render

A command receives the merged contributions of the resolved steps in sequence order, rewritten for the workspace by `Contributions::environment_for`. The backend owns exactly three variables: HOME at the workspace, TMPDIR at a directory inside it, and PWD at the workspace. PATH is exactly what the steps contribute; the backend does not assemble one, so a sandbox with no steps has no PATH. The backend sends the merged environment over the socket at create, and the holder sets each child's environment to it exactly. No value reaches argv, and the transport is the socket rather than an environment file, so no value is written to disk. Nothing from the caller's environment reaches the command. The workspace temporary directory is created before the first command. Provenance records the three backend variables as set.

**Slug:** `environment-render`

### FR-006: Environment: Withhold Ungranted Values

The backend judges each variable the way the Linux backend does. A path-list variable keeps its elements that name the workspace or a granted path; one with no granted element left is withheld. Any other variable whose value is one absolute path outside the workspace and the grants is withheld. A withheld variable is recorded as removed by name and is never spawned. A value is not a grant: a variable naming an ungranted path is inert and the path stays denied. The backend does not maintain a credential-name table of its own. The profile's allowlist is the control: a value is not a grant, so a path that was never granted stays unreadable whether or not the variable names it. The documentation says that a secret exported under an ordinary name is passed, because the backend matches the form of a path and not the intent of a hook.

**Slug:** `environment-withholding`

### FR-007: Workspace: Direct Path

macOS has no mount namespace, so the sandbox workspace is the backend's state directory path and is not bound anywhere. create copies the staged workspace into `data_dir/litterbox/seatbelt/<project-slug>/<sandbox-name>/workspace`, records that absolute path, and every command runs with it as the working directory. upload writes only inside it and download reads only from it; both open each component without following a link, and a symlink, device or FIFO the sandbox planted is refused and named. create keeps a symlink in the staged tree when its target stays inside the tree, and refuses one that leaves it; a device or FIFO is always refused. The workspace is the only host tree the profile makes writable, apart from `/dev/null`. The backend does not create a home directory outside the workspace.

**Slug:** `workspace-direct`

### FR-008: Exec: Holder Request

exec connects to the sandbox's holder socket and sends the command argv. The holder forks, the child applies the profile and execs with the sandbox environment and the workspace as its working directory, and the holder captures standard output and standard error under the same cap as Docker and returns them with the exit status. The connect and send are bounded; the response stream is not, because a command may be silent for as long as it runs. An apply error is reported as a failure naming the `sandbox_init_with_parameters` message, a policy error as Misconfigured naming the helper's reason, an exec error as a failure naming the command that could not be executed, and any exit status is the workload's own. A command is never run unconfined because the profile failed. While a sandbox is recorded paused, exec is refused as Denied until resume.

**Slug:** `exec-helper`

### FR-009: Lifecycle: Holder Process Group

create calls setsid in the holder, so the holder is the sandbox's session and process group leader, and records its pid and start time. If setsid fails, create fails: a holder in the server's group would let pause or delete signal the server. Every command is a child of the holder and stays in that group unless it calls setsid, including a server whose invoking shell has exited. pause sends SIGSTOP to the group and inspection reports Paused from the process state. resume sends SIGCONT and inspection reports Active. delete sends SIGKILL to the group when the holder pid matches the record and waits up to two seconds for the leader to go before removing state; when the pid is absent but the state directory exists, it logs that the group is orphaned. If the pid exists with a different start time, delete does not signal the group; it removes the state directory and logs the refusal. Pause and resume signal only after the holder pid exists, is not a zombie, and matches the recorded start time; delete is the exception, because confined children must not survive the record. Inspection reports Paused from the process state without asking a stopped holder; when the record is active it sends Status with a one-second deadline. A command that called setsid leaves the group, stays confined, and is not addressable by pause or delete; the documentation states that this is accepted because there is no pid namespace.

**Slug:** `lifecycle-group`

### FR-010: Record: Payload

The backend writes its own `SandboxRecord.data` payload and every later operation reads it. The payload holds: a version, the helper path, the holder pid and start time, the socket path, the host workspace path and the sandbox workspace path, the grants as rendered and the collapsed ancestors, the environment provenance, the network posture and the tag. It holds no forwards because the backend has no forwarding facet. A higher version is refused as unreadable; a lower one, or a payload missing a required field, is refused with a remedy of deleting and recreating the sandbox. Backend state lives at `data_dir/litterbox/seatbelt/<project-slug>/<sandbox-name>/`, holds the workspace and the holder log, and keeps the metadata store's file mode. The socket path is recorded and lives in the user temp directory. A record that cannot be read is still deletable.

**Slug:** `record-payload`

### FR-011: Readiness

readiness reports one check for each requirement, each with a state, a reason and a remedy. The host is macOS. `sandbox_init_with_parameters` resolves through `dlsym`. The helper binary exists, is a regular file after resolution, is owned by root or the effective uid, and is not group or world writable. A probe helper runs with `(version 1)(deny default)`, exits 0, and never confines the caller; its `sandbox_init_with_parameters` message is the remedy text on failure. The state root's parent is writable. No check confines the caller, starts a sandbox or runs a command. create refuses on any check that is not available, and a caller that reaches the backend without the registry's gating still cannot create a sandbox on an unusable host.

**Slug:** `readiness-checks`

### FR-012: Egress: Two Postures

The section selects one of two postures. With `network = 'denied'`, which is the default, the profile emits no `network-outbound` rule, so a command cannot connect to any host or port, including loopback. With `network = 'host'`, the profile emits `(allow network-outbound)` unqualified, because the language accepts only `*` and `localhost` as a host token, so it cannot name another destination. The renderer also accepts a list of loopback ports and emits one `(remote ip 'localhost:<port>')` rule for each, which is the rule T-021 established for the proxy. This ADR starts no proxy and exposes no configuration key for that list, so the proxy ADR adds the posture that uses it. Inspection reports Denied or Direct and creates no proxy, socket or route in either case. The contract's `Proxied` posture holds an opaque endpoint; this ADR never reports it, and the proxy ADR supplies it. A project that declares ports is refused at create as Unsupported, naming the missing `network-bind` grant, and leaves no state directory.

**Slug:** `egress-posture`

### FR-013: Invariants: Allowlist Access

The profile grants only the baseline, the workspace and the validated contributions; it emits no explicit deny rule. A deny list would be a denylist inside an allowlist design: it names the credential files it knows about and says nothing about the rest, so a file missing from the list is indistinguishable from a granted one. Instead the mount validator refuses a contribution under `/Users` or the current user's home, the workspace or its ancestor, the backend's state directory or the metadata directory. A home path never reaches the profile, so no credential file is reachable and `(deny default)` is the only deny. There is no config key with which a machine can widen this.

**Slug:** `allowlist-access`

### FR-014: Diagnostics: Three Failures

The caller can tell a setup failure, an apply failure and a workload failure apart. A contributed path that fails validation is a create failure naming the rule and the path. An apply failure is an error frame with the `sandbox_init_with_parameters` message verbatim. A profile the helper refuses by policy is an error frame with its reason, reported as Misconfigured. A command that cannot be executed is an exec error. The workload's own status is returned unchanged. The deny rule sets a per-sandbox tag, so an operator can find a denial with `log show --info --debug` and the tag. create logs the helper path, the socket and the log at debug level, and never an environment value. A setup failure reaches the doctor as Unavailable with the requirement named.

**Slug:** `setup-diagnostics`

### FR-015: Documentation: Configuration

The backends page and the user configuration reference name seatbelt, its four keys (`network`, `baseline`, `baseline-reads`, `mach-services`) and their defaults, its readiness output, what it applies from a devshell step, that it configures no home, what the network postures allow, that it refuses declared ports, that it has no resource limits, and that a sandbox with no provisioning step has no PATH. The page stops describing Docker as the only mechanism and states that the mechanism does not need Nix. It says the backend has no knowledge of Xcode or any toolchain: a project that needs a compiler contributes the read paths, and a toolchain's cache warnings from the host temp directory are tolerated. What the documentation says the sandbox can reach matches what inspection reports.

**Slug:** `backend-documentation`

### FR-016: Toolchain: Not the Backend's Concern

The backend knows no toolchain. It does not run `xcode-select`, does not grant an Xcode installation, and exposes no `toolchain` key. A project that needs a compiler contributes the installation root and the licence state as read mounts; the ordinary mount validation accepts them and the backend records them like any other grant. A compiler that writes a cache to the host user temp directory gets warnings and still succeeds, and the backend does not grant a writable host path for it. This keeps the mechanism independent of a host's toolchain layout, its licence policy and whether the host has Nix.

**Slug:** `developer-directory`

## Non-functional Requirements

### NFR-001: Fail Closed

The backend refuses before a command runs when a condition is unsafe: `sandbox_init_with_parameters` missing, the probe failing, the helper missing or untrusted, a workspace that cannot be created, a contributed path that breaks validation, a profile over the size limit, a collapse that would grant `/`, a record that is unreadable or from another version, a group whose leader is stale or foreign, and a paused sandbox asked to exec. No path falls back to an unconfined command, and no sandbox starts from a partly rendered profile. A refusal names the requirement, the reason and a remedy that does not suggest disabling the sandbox.

**Slug:** `fail-closed`

### NFR-002: Audit Surface

The helper is single-threaded, has no async runtime, no configuration and no network beyond one unix socket, and needs no dependency beyond libc. It does not link the rest of the crate. One module renders the profile, and the backend supplies only validated arguments. The holder forks confined children and makes no policy decision per command. The holder itself runs unconfined, is backend-owned, and never executes workload code; the confinement boundary is the child it forks. A reviewer can read the helper and the renderer in one sitting; the promoted helper builds in under a second, as the prototype did.

**Slug:** `audit-surface`

### NFR-003: Golden Profiles

Tests assert the exact profile text the helper renders for the empty request, for a request with an environment and two read grants, for a collapsed closure, and for each network posture. Each test names the invariants it checks: deny default, the root literal, no store path, process and signal rules scoped to the same sandbox, the enumerated mach services, writes limited to the workspace and `/dev/null`, no explicit deny rule, and no `network-bind`, `network-inbound` or unix-socket rule. Every golden case also asserts the parameter table, so a path change is a table diff rather than a text diff. A second test proves that removing any invariant changes the profile, so deleting a rule means editing a test. The profile is never rendered from a caller-supplied string.

**Slug:** `golden-profiles`

### NFR-004: Cost

Applying a profile and execing costs about 12ms for the prototype's small profile. The empty request stays in that band. Measured on this host: 64 grants cost 11.7ms median, 100 grants 17.0ms, and 238 grants 42.7ms, which is why the collapse threshold is 64 rather than a target. There is one holder process per sandbox, alive between commands, and one process tree per command. create pays the workspace copy and the holder start.

**Slug:** `cost`

### NFR-005: Platform Requirements

The backend runs on macOS only. It needs the `sandbox_init_with_parameters` symbol, which is deprecated and may disappear, and a trusted helper. It needs no root, no entitlement, no Developer ID, no notarisation, no virtual machine and no container runtime. It does not require Nix. The registry does not list it on another platform, and no other backend changes.

**Slug:** `platform-requirements`

### NFR-006: Test Coverage

The renderer, the collapse rule, the environment merge, the record payload and the readiness checks are unit-testable without a sandbox through a host seam, as the Linux backend does. A host suite runs on macOS and skips with the missing requirement named elsewhere. It covers a sandbox with no steps running a command, a write outside the workspace denied, a credential read denied and a descendant confined. It covers create, two commands, a background command whose shell exits, pause, resume, delete and the setsid escape, plus a holder that dies out of band and a concurrent pair of commands. Tests assert exact exit codes rather than any non-zero status, pair every denial with a positive control, and scope process checks so they cannot match the test process. A contract change runs both backends' suites. The golden profile tests run everywhere.

**Slug:** `test-coverage`

## Acceptance Criteria

### AC-001: Backends Listing

On macOS `litterbox backends` lists seatbelt beside docker and bubblewrap, marks the selected one, and prints each readiness check with its state and, where it is not available, a reason and a remedy. `--json` emits the same fields the other backends emit.

**Slug:** `backends-listing`

### AC-002: No DevShell

With seatbelt selected and a project with no provisioning step, create succeeds and `litterbox shell <name> -- /bin/sh -c 'echo ok'` prints ok. The command sees HOME, TMPDIR and PWD only, and no PATH. A second sandbox in a project that declares a devshell step sees the contributed environment, and its PATH leads with the devShell's directories.

**Slug:** `standalone-sandbox`

### AC-003: Isolation

A command writing outside the workspace fails with EPERM and no file appears. A command reading a host credential path fails; a contribution cannot grant one, because the validator refuses every path under a home directory. A Nix binary is not runnable without a store grant and is runnable when the contribution grants the store. A descendant process is confined by inheritance.

**Slug:** `isolation-holds`

### AC-004: Closure Collapse

A devshell whose closure is 238 paths creates without error, the recorded grants show one collapsed ancestor rather than 238 entries, and the first command runs without a retry. A contribution whose common ancestor is `/` is refused as Misconfigured and leaves no state directory. A profile that would exceed 60 KiB is refused before `sandbox_init_with_parameters` runs.

**Slug:** `closure-collapse`

### AC-005: Environment

A contributed variable with a build flag reaches the command with its value unchanged. A variable whose value names an ungranted host path is absent and inspection reports it as withheld by name. A workspace-rooted value names the sandbox workspace. No value from the caller's environment reaches the command.

**Slug:** `environment-exact`

### AC-006: Lifecycle

A command that appends to a file every 50ms stops after pause, inspection reports Paused, it continues after resume, inspection reports Active, and delete leaves no holder, no command in its group, no state directory and no workspace. A server started by one command, whose shell exits, is stopped by pause and killed by delete. Exec while paused is refused. A `setsid` child survives delete, stays confined, and is named in the documentation as unaddressable.

**Slug:** `lifecycle-works`

### AC-007: Stale Holder

A holder whose pid is absent is treated as gone by pause and resume. Delete signals the recorded group when the pid is absent and the state directory exists, logging it as orphaned. A recycled pid with a different start time is never signalled; delete removes the state and logs the refusal. A sandbox whose workspace was removed out of band reports Missing and is still deletable. An active holder whose socket does not answer reports Error, not Active.

**Slug:** `stale-group`

### AC-008: Readiness

On a host with the helper removed, readiness reports the helper check Unavailable with a remedy and create refuses. On a host where the probe profile is rejected, readiness reports the probe's `sandbox_init_with_parameters` message and create refuses. The server is never confined by a readiness or create check.

**Slug:** `readiness-refuses`

### AC-009: Egress

With the default denied posture, a command cannot connect to any address, loopback included. With `network = 'host'`, the same command reaches the host's network and inspection reports Direct. A profile rendered with a loopback port list contains the `localhost` rule for that port and no wildcard rule. A project that declares a port is refused at create as Unsupported and leaves no state directory.

**Slug:** `egress-postures`

### AC-010: Docker and Bubblewrap Unchanged

The existing suite and `arch-lint check` pass. DEFAULT stays docker. A machine that holds records from more than one backend lists the selected backend's sandboxes, reports the others as another backend's, and does not fail.

**Slug:** `other-backends-unchanged`

### AC-011: Diagnostics

A denial appears in the kernel log with the sandbox tag from `log show --info --debug`, and the helper sets errno EPERM in the command. A profile that fails to apply returns an error frame with the `sandbox_init_with_parameters` message and does not run the command. A profile refused by policy returns a policy error and is reported as Misconfigured. A missing command returns an exec error. A workload failure returns its own status.

**Slug:** `denial-diagnostics`

### AC-012: Toolchain Contributed

On this host a command that compiles C succeeds when the project contributes the Xcode installation root and licence plist as read mounts, and fails with a named denial when it does not. The profile contains no path the backend discovered, and a host without Xcode creates sandboxes normally.

**Slug:** `developer-directory-optional`

## Edge Cases

### EC-001: Symbol Removed

A macOS that no longer exports `sandbox_init_with_parameters` reports readiness Unavailable and create refuses. Nothing falls back to running unconfined, and the remedy names the platform change rather than suggesting an entitlement build.

**Slug:** `symbol-removed`
### EC-002: Profile Rejected

A profile the language rejects returns an apply error with the `sandbox_init_with_parameters` message and the command does not run. A profile above 60 KiB is refused by the helper as a policy error before it is applied, and the backend reports Misconfigured with the size, because one measured over-size profile aborted on a libsandbox assertion instead of returning an error.

**Slug:** `profile-rejected`
### EC-003: Setsid Escape

A command that calls `setsid` leaves its group. It stays confined, because the profile is inherited and cannot be dropped, but pause and delete cannot address it. The backend does not scan for it and does not claim to have killed it.

**Slug:** `setsid-escape`
### EC-004: Pid Reuse

A holder pid that now belongs to another process is refused by the start-time check and is never signalled. The holder is the only process the backend signals by identity; commands are reached through its group.

**Slug:** `pid-reuse`
### EC-005: Symlinked Contribution

A contributed path that resolves through a symlink outside the grant is refused at create and named. `/tmp` resolving to `/private/tmp` is the ordinary case and is handled by `realpath`, not by a special rule.

**Slug:** `symlinked-contribution`
### EC-006: No Toolchain Contributed

A project that contributes no toolchain creates sandboxes normally. A command that needs a compiler fails with the toolchain's own error, and the profile holds no developer path.

**Slug:** `no-xcode`
### EC-007: Toolchain Licence

A project that contributes the Xcode installation but not its licence state reaches the toolchain's licence error. The sandbox is not reported as broken, and the remedy is the licence command, not a profile change.

**Slug:** `xcode-licence`
### EC-008: Compiler Cache

A compiler that writes to the host user temp directory reports cache warnings and still builds; T-053 measured exit 0. The backend grants no writable host path for it, and the documentation does not claim that a compiler never writes outside the workspace.

**Slug:** `compiler-temp`
### EC-009: No Steps

A project with no provisioning step has a sandbox that runs only what the platform baseline provides. Its first command fails with the shell's own error when the command is not in the baseline, and no PATH is invented to hide that.

**Slug:** `no-steps`
### EC-010: Concurrent Commands

Two commands run at once through the holder, and neither sees the other's output or status because each has its own pipes and its own child. Pause stops both.

**Slug:** `concurrent-commands`

