# Cartography map

**Destination:** Nix development shells run as first-class sandboxed environments on Linux and macOS, confined in user space by bubblewrap and App Sandbox respectively, with no virtualisation

Created: 2026-10-31

## Summary

| Metric | Value |
|--------|-------|
| Tickets | 26 |
| Answered | 26 |
| Open | 0 |
| Probing | 0 |
| Abandoned | 0 |
| Blocked | 0 |
| Frontier | empty |

## Tickets

### T-001: Can the App Sandbox entitlement API confine an unsigned devShell binary?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No for the entitlement API: it requires a signed bundle. The spike exercised the sandbox_init entry point with a Seatbelt profile |
| Evidence | [evidence/T-001.md](evidence/T-001.md) |

### T-002: Does the sandbox_init entry point confine an unsigned binary with a Seatbelt profile on a current macOS?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. sandbox_init applies a Seatbelt profile and confines the adhoc-signed binary; this is not entitlement-based App Sandbox support |
| Evidence | [evidence/T-002.md](evidence/T-002.md) |

### T-003: Is bwrap viable as an unprivileged Linux sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. It builds the sandbox from unprivileged user namespaces, with no root and no daemon |
| Evidence | [evidence/T-003.md](evidence/T-003.md) |

### T-004: Can the host Nix store be shared without copying?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes for integrity and deduplication: a read-only bind exposes the same immutable paths. Confidentiality is not implied |
| Evidence | [evidence/T-004.md](evidence/T-004.md) |

### T-005: Can bwrap restrict egress by destination?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. An unshared network namespace has loopback only; host filtering needs a proxy |
| Evidence | [evidence/T-005.md](evidence/T-005.md) |

### T-006: Does the macOS sandbox require a separate kernel or virtual machine?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. It enforces per process in the shared kernel, so a kernel exploit defeats it. macOS needed no VM, no image and no container runtime. The bwrap half of the same question is covered by T-003: unprivileged user namespaces, no root, no daemon |
| Evidence | [evidence/T-006.md](evidence/T-006.md) |

### T-007: Can lifecycle work without a supervisor?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Partially: process groups give group-local pause, resume and kill. A setsid child survives destroy, stays confined but can still use CPU or write granted paths. Superseded on Linux by T-013 and T-019: a pid-namespace holder kills every descendant, and pid-namespace enumeration pauses them |
| Evidence | [evidence/T-007.md](evidence/T-007.md) |

### T-008: Do the target distributions permit unprivileged user namespaces?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes on NixOS and Fedora; Ubuntu 24.04 blocks by default and allows when the AppArmor and clone sysctls are relaxed. Verified in VMs |
| Evidence | [evidence/T-008.md](evidence/T-008.md) |

### T-009: Can lifecycle use --new-session with cgroup v2 freeze on systemd hosts and process-group signalling elsewhere?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes on systemd hosts with cgroup delegation: a transient user scope with freeze/thaw. The process-group launcher is the fallback |
| Evidence | [evidence/T-009.md](evidence/T-009.md) |

### T-010: Can per-sandbox resource limits be applied without a long-lived supervisor?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes on systemd: MemoryMax and TasksMax on the transient scope. The fallback runs unlimited |
| Evidence | [evidence/T-010.md](evidence/T-010.md) |

### T-011: Can the host-side devShell capture supply the environment allowlist?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes, but only by a differential capture under a sanitized caller baseline: the 85 exported variables from nix print-dev-env plus the 16 the shellHook adds or changes. Sanitising matters because the hook embeds the caller's RUSTFLAGS and prepends to the inherited PATH; validated afterwards, PATH leads with CARGO_HOME/bin, RUSTFLAGS carries no caller data, and a joined command sees 93 variables against the 91 captured, the two being LC_CTYPE and PWD that the shell sets. Copying nix develop's environment instead carries the caller's SSH, D-Bus, Docker and Wayland sockets, and loses 109 bash functions either way |
| Evidence | [evidence/T-011.md](evidence/T-011.md) |

### T-012: Does a per-exec bwrap sandbox share pid and network context with a later command?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. Each invocation creates its own pid, net and mount namespaces; only the workspace, a bind-mounted host directory, is shared |
| Evidence | [evidence/T-012.md](evidence/T-012.md) |

### T-013: Can an unprivileged process join a live bwrap sandbox, so successive commands share pid and network context without a supervisor of our own?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: setns into the holder works unprivileged on the same uid. The addressable pid is the child-pid bwrap reports on its status fd, not the caller's. Joined commands share pid, net and mount context and the host cannot reach the sandbox loopback. Killing the holder kills the whole pid namespace, including setsid escapees |
| Evidence | [evidence/T-013.md](evidence/T-013.md) |

### T-014: Where must an egress proxy sit to be reachable from a bwrap sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-012 |
| Answer | Outside the sandbox, reached through a bind-mounted unix socket: the network namespace has no route to the host, but a unix socket is a file. One host-side proxy can serve every sandbox and stays out of the confinement |
| Evidence | [evidence/T-014.md](evidence/T-014.md) |

### T-015: Can the nix devShell evaluate and build on aarch64-darwin, so a macOS CI job reuses the existing devShell?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. The Darwin spike was built and run on macOS 27.2 on Apple silicon, every prototype invoked through nix develop ./nix. The devShell only proves itself, not the rest of the workspace's crates |
| Evidence | [evidence/T-015.md](evidence/T-015.md) |

### T-016: Does a host-captured devShell environment produce a working build inside the bwrap sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-011 |
| Answer | Yes. cargo build and cargo test ran inside the holder with a read-only store, no network and a private home, using the 91-variable allowlist; the workload sees 93, the extra two being LC_CTYPE and PWD from the shell, and nothing is dropped. Only references to the workspace move if it is bound elsewhere: /nix/store is mounted at the same path, so store paths stay valid |
| Evidence | [evidence/T-016.md](evidence/T-016.md) |

### T-017: Does a host-captured devShell environment produce a working build inside the macOS sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-011 |
| Answer | Yes, with no baseline addition. cargo build and cargo test pass with the captured environment once HOME, TMPDIR, TMP, TEMP and NIX_BUILD_TOP are repointed into the workspace; NIX_BUILD_TOP must stay set because the clang wrapper reads it unguarded, and the store paths plus metadata ancestors the profile already grants are enough. 524 denials were logged and every one is tolerated: metadata on /nix and the cryptex and firmlink aliases, vfs.disk-space, /dev/dtracehelper, /dev/tty and notification-centre shm |
| Evidence | [evidence/T-017.md](evidence/T-017.md) |

### T-018: Can a bwrap sandboxed command invoke nix at all, and what does granting that give it?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | It executes, but is useless without the daemon socket and network, and the socket gives a confined workload demonstrable authority it should not have: it grew the host store with a root-owned, host-executable path, and the daemon accepts build requests that run outside this sandbox. That is host-store growth and out-of-sandbox execution, not mutation of existing immutable paths; T-004 already names the daemon as a write channel. Withhold it and keep nix outside the sandbox |
| Evidence | [evidence/T-018.md](evidence/T-018.md) |

### T-019: Can commands joined into a holder be frozen and limited when they are started outside the holder's cgroup?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-013 |
| Answer | Yes, two ways. Cgroups are the primitive for pausing: assign each joined pid to the sandbox scope before exec, and freeze covers every command atomically. Without cgroups, pause is a stop-and-rescan fixpoint over pid-namespace members; a one-pass sweep is not a pause, because a forking workload escapes it (measured), and a same-uid peer can livelock the fixpoint by signalling its stopped peers. cgroup.kill tears the whole scope down atomically. Delegation lets a hostile same-uid workload move itself out of the scope, so freeze and limits are resource management, not a boundary |
| Evidence | [evidence/T-019.md](evidence/T-019.md) |

### T-020: Does the macOS sandbox share pid and network context with a later command?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. It creates no namespace, so every command runs in the host pid space and on the host network stack, and cross-command client traffic works. Sandboxed listener reachability is untested: server operations are policy-gated too, and the shipped profile emits neither network-bind nor network-inbound |
| Evidence | [evidence/T-020.md](evidence/T-020.md) |

### T-021: Where must an egress proxy sit to be reachable from the macOS sandbox, given the profile accepts only localhost and * as a host token?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-020 |
| Answer | Loopback TCP, on a granted port. A real host proxy answers HTTP/1.0 200 OK with --connect localhost:<port> and returns EPERM without it. A unix socket inside the workspace is not reachable even though the file is 0666 and the workspace is granted: connect(2) on AF_UNIX is network-outbound with a remote unix-socket filter, and the process also needs system-socket for AF_UNIX, neither of which the profile emits. The Linux bind-mounted-socket placement does not carry over, because macOS has no network namespace and loopback is already on the host stack |
| Evidence | [evidence/T-021.md](evidence/T-021.md) |

### T-022: Can the bind set be derived from the devShell's Nix closure instead of discovered by failure?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. The 238-path closure of the devShell environment, bound one path at a time, ran the same cargo test that the whole-store bind ran. The host store holds 57,534 paths, so the view shrinks by two orders of magnitude, and the set tracks the flake instead of a hand-maintained list. It is derived, not minimal: it carries Rust std libs for Windows and Apple targets that this workload never uses |
| Evidence | [evidence/T-022.md](evidence/T-022.md) |

### T-023: Does the implemented provider contract carry the devShell's store closure to the backend that binds it?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. CreateRequest.environment carries variables only (EnvironmentSource::Captured), Workspace a host directory, and the orchestrator hardcodes EnvironmentSource::Mechanism; the T-022 closure has no field. The bwrap backend can bind all of /nix/store, which T-004 and T-016 proved sufficient, until the capture ADR amends the contract |
| Evidence | [evidence/T-023.md](evidence/T-023.md) |

### T-024: Does a holder survive the one-shot CLI process that created it, so a later process can join?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-013 |
| Answer | Yes. A holder spawned and abandoned by a bash -c launcher stayed alive, and a later process joined it by bwrap's child-pid, both with and without --die-with-parent; that flag ties the sandbox command to bwrap, not bwrap to the launcher. The backend must still detach the holder's standard streams, which this probe left attached |
| Evidence | [evidence/T-024.md](evidence/T-024.md) |

### T-025: How should a host select a mechanism when the project's environment is mechanism-neutral?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | The machine-owned local override selects it today. Automatic readiness-ordered selection is deferred to its own ADR (F), because it trades the user's freedom to configure a machine against the project's need for a consistent environment |
| Evidence | adrs/2026-11-03-sandbox-provider-contract/spec.toon (FR-021) |

### T-026: Can a project name a backend to say which provisioning steps run against it without selecting that backend?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. Naming a backend declares compatibility; selection stays in the machine-owned local override. FR-021's never-names-a-backend becomes never-selects-a-backend, and a selected backend the project does not list is reported rather than silently running without its steps |
| Evidence | adrs/2026-11-03-sandbox-provider-contract/spec.toon (FR-021, EC-004) |

## Decision

Adopt bubblewrap on Linux and App Sandbox via the sandbox_init entry point on macOS. The entitlement API requires a signed bundle, so unsigned devShell binaries use the deprecated entry point; its longevity is a carried risk. Egress beyond loopback needs a proxy on both platforms, and the two platforms do not share a transport: macOS reaches it as loopback TCP on a granted port (T-021), Linux through a bind-mounted unix socket outside the network namespace (T-014), so the network facet is one endpoint per platform rather than one shape. On Linux the sandbox is a holder process (the pause-container shape, as Flatpak enter does it): bwrap runs a sleeping child in the pid, net, mount and user namespaces, and every command is a setns join from outside. That restores shared pid and network context, gives pause and resume a sandbox-level meaning through the holder's cgroup, and closes the setsid escape, because killing pid 1 kills the whole pid namespace. T-013 is proven unprivileged on the same uid. T-015 is answered: the devShell builds on aarch64-darwin. T-019 is answered: joined commands are assigned to the sandbox scope before exec, so freeze and limits cover every command; without cgroups, pause is a stop-and-rescan fixpoint that a one-pass sweep does not implement and that a same-uid peer can livelock, so cgroups are the primitive and enumeration is the fallback. On macOS there is nothing to hold open: the profile is a per-process property and commands already share the host's pids and network (T-020), so the backend is a per-command helper. Carried risks: a user manager without cgroup delegation makes the scope start fail and the caller selects the pid-namespace fallback, which still closes the setsid gap but cannot bound memory or task count; cgroup delegation is not a security boundary against a hostile same-uid workload. The destination is now proven on both platforms: the devShell environment is captured on the host by differential probe (T-011) and a devShell build runs inside the holder with no network and a read-only store (T-016). nix is deliberately unavailable inside the sandbox, because the daemon socket lets a workload write to the host store (T-018). On Linux egress is a host-side proxy reached over a bind-mounted unix socket, so the sandbox needs no route at all (T-014). The store view is the devShell's 238-path closure rather than the whole 57,534-path store, derived from Nix rather than discovered by failure, and the same test passes against it (T-022). T-017 is answered: a devShell build runs inside the macOS sandbox with the captured environment and needs no baseline addition, though the capture must be sanitised of build-sandbox scratch paths and NIX_BUILD_TOP must stay set for the clang wrapper. T-021 is answered: the macOS proxy is a loopback TCP listener on a granted port, because a unix socket is not reachable without AF_UNIX rules the profile does not emit. Still unproven: a devShell whose closure exceeds the toolchain closure. Revised on the store view: bind the realised closure by default after validating a multi-user, root-owned Nix installation, with the whole store as an explicit option for workloads that reach undeclared paths; the closure covers declared dependencies only, so the wide bind is the fallback.

