# Cartography map

**Destination:** Nix development shells run as first-class sandboxed environments on Linux and macOS, confined in user space by bubblewrap and App Sandbox respectively, with no virtualisation

Created: 2026-10-31

## Summary

| Metric | Value |
|--------|-------|
| Tickets | 55 |
| Answered | 55 |
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
| Answer | Yes, since devshell-environment-implementation: prepare fills Contributions.mounts with the T-022 closure, ResolvedStep carries it, and CreateRequest.provision delivers it. StoreView::Whole still collapses it to /nix/store. Rendering the list is the bwrap and Darwin backends' work |
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

### T-027: Where should a provisioning kind live: the contract's closed ProvisionKind enum, or a registry outside the contract?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | A compiled-in registry outside the contract. A step carries identity, kind and the settings that kind owns, and the backend receives the contributions instead of the step |
| Evidence | [evidence/T-027.md](evidence/T-027.md) |

### T-028: What should a backend report about a step: support per kind, or support for the contributions it applies, and is that report static or derived from readiness?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-027 |
| Answer | Static support for contribution sorts: an environment and read-only mounts. Readiness stays dynamic and host-specific, so a mechanism's ability to apply a contribution is a compile-time property |
| Evidence | [evidence/T-028.md](evidence/T-028.md) |

### T-029: Should the exported-but-empty build-phase variables from print-dev-env be contributed, or dropped?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Contributed. nix develop and nix-direnv both leave them exported, so dropping them diverges from the development shell. This checkout has 18, not the 11 the prototype findings recorded |
| Evidence | [evidence/T-029.md](evidence/T-029.md) |

### T-030: Which variables should the step contribute: a denylist of shell and Nix variables, or the print-dev-env script's assignments with the six Nix temp variables restored as nix-direnv does?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | The nix-direnv rule. Evaluate the print-dev-env script once in the same shell, then restore NIX_BUILD_TOP, TMP, TMPDIR, TEMP, TEMPDIR and terminfo. HOME, PWD, OLDPWD, SHLVL and _ are already in the caller, so they never become contributions |
| Evidence | [evidence/T-030.md](evidence/T-030.md) |

### T-031: Who writes the prepared environment and store view into the sandbox record, and in what shape?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | The backend writes its own SandboxRecord.data payload and reads it in exec, inspect and delete. The contract requires the same environment and mounts on later commands and provenance on inspect, and defines no payload shape |
| Evidence | [evidence/T-031.md](evidence/T-031.md) |

### T-032: What does the kind field in per-step provenance hold once kinds leave the contract?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-027 |
| Answer | The registry's kind name as a string. The contract keeps identity and the kind name for attribution and no longer names a closed kind type |
| Evidence | [evidence/T-032.md](evidence/T-032.md) |

### T-033: Does the host sleep work as the holder command bound into the view?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. sleep with a large integer duration holds the sandbox; no GNU infinity spelling is needed |
| Evidence | [evidence/T-033.md](evidence/T-033.md) |

### T-034: Does a killed client leave a zombie inside the sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. The command is reparented outside the pid namespace and reaped there; zero defunct processes remained after the orphan exited |
| Evidence | [evidence/T-034.md](evidence/T-034.md) |

### T-035: Does a closure larger than 238 paths bind and start?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. 1779 individual binds started in 69ms and ran a command; bwrap merged them into 52 mount entries, so a mount table check must verify coverage rather than one entry per path |
| Evidence | [evidence/T-035.md](evidence/T-035.md) |

### T-036: Do namespace inodes stay stable while the sandbox lives?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. All seven inodes were identical after three joins, so a record can check them before every join, paired with the process start time |
| Evidence | [evidence/T-036.md](evidence/T-036.md) |

### T-037: Does this host offer cgroup v2 with a usable user scope?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. cgroup2fs, a user manager and a transient scope that starts |
| Evidence | [evidence/T-037.md](evidence/T-037.md) |

### T-038: Does setns into the sandbox cgroup namespace work unprivileged?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. The joined process saw the holder inode, so a command shares all seven namespaces |
| Evidence | [evidence/T-038.md](evidence/T-038.md) |

### T-039: Does bwrap's status document mean the sandbox is usable?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. The document arrives early in setup: 43ms against 4246ms to a usable 1779-path view, and a join before the view is built fails. create must wait for the recorded mounts, not for the document |
| Evidence | [evidence/T-039.md](evidence/T-039.md) |

### T-040: Does every recorded mount present mean the sandbox is usable?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-039 |
| Answer | Yes. In four runs the last recorded mount appeared 28 to 34ms before the first join succeeded, with the old root already detached, so create can poll the recorded mounts |
| Evidence | [evidence/T-040.md](evidence/T-040.md) |

### T-041: Can a process join the sandbox in-process, and in what namespace order?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-013 |
| Answer | Yes, but not in nsenter's order. The sandbox's namespaces are owned by its user namespace's parent, so the join enters that parent first, then ipc, net, pid, mnt, uts and cgroup, then the sandbox user namespace. Joining the sandbox user namespace first grants capabilities there and every later join fails with EPERM |
| Evidence | [evidence/T-041.md](evidence/T-041.md) |

### T-042: Does the contract define the environment of a sandbox with no provisioning step, and does the Linux backend already implement it?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. An empty step list yields exactly HOME, TMPDIR and PWD with origin Default and no PATH: the backend does not invent a search path, so a mechanism stands alone and a devShell is optional |
| Evidence | crates/agentkit-litterbox/src/backends/bubblewrap/environment.rs (no_step_means_no_path); crates/agentkit-litterbox/src/sandbox/contract.rs (Contributions::default) |

### T-043: Can Seatbelt confine a sandbox whose only inputs are a workspace and a platform baseline, with no Nix store grant and no devShell contribution?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A deny-default profile with a platform baseline and the workspace confines /bin/sh and denies out-of-workspace reads and writes with no /nix grant; a Nix binary aborts on its ungranted store dylib. The root literal read rule is required: without it every exec dies with SIGABRT and no output |
| Evidence | [evidence/T-043.md](evidence/T-043.md) |

### T-044: Does a profile carrying the devShell closure's hundreds of read grants compile and apply inside the per-command cost budget?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No for the closure shape. 238 grants apply but cost about 43ms per command against 12ms for one, 1779 are rejected, and the 65535-byte profile limit is reached near 1010 grants; cost is superlinear before that. The backend must collapse mounts to a common ancestor rather than render one rule per path |
| Evidence | [evidence/T-044.md](evidence/T-044.md) |

### T-045: Can a later, unrelated process pause, resume and kill a sandbox from its recorded process group, and does a setsid child survive?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A later process stops, continues and kills the group after the launcher has exited. A setsid child survives the group kill, stays confined and is unaddressable by group, so the record needs the pgid plus a reuse check |
| Evidence | [evidence/T-045.md](evidence/T-045.md) |

### T-046: Is the Xcode developer directory a platform baseline path or a toolchain contribution, given that it can itself resolve into the Nix store?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-043 |
| Answer | A toolchain contribution discovered at runtime, never a baseline constant. The grant unit is the Xcode installation root plus the licence plist and a writable user temp dir; xcode-select -p may name /Applications, CommandLineTools or a store path, and Contents/Developer alone fails on sibling frameworks |
| Evidence | [evidence/T-046.md](evidence/T-046.md) |

### T-047: Can the backend supply the contributed environment to the helper, so the helper has no allowlist of its own?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A transparent helper passes every contributed variable unchanged, while the prototype's allowlist drops five of eight, including RUSTFLAGS and a credential. Withholding values that name ungranted paths stays the backend's job |
| Evidence | [evidence/T-047.md](evidence/T-047.md) |

### T-048: Can readiness report whether Seatbelt applies without confining the calling process?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes, with dlsym plus a probe child. The symbol resolves but only applying a minimal profile proves the mechanism works, and a bad profile returns a message rather than an errno. The probe costs 7.6ms median and never confines the caller |
| Evidence | [evidence/T-048.md](evidence/T-048.md) |

### T-049: Is applying Seatbelt in a forked child of a threaded server safe, or does it mandate a separate helper binary?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Fork+apply survived 400 of 400 in a four-thread process with no hangs or crashes, but POSIX permits only async-signal-safe calls before exec and the profile is per-process. The helper is still mandated: applying in the server would confine the server, and the helper is not slower |
| Evidence | [evidence/T-049.md](evidence/T-049.md) |

### T-050: Does the Darwin provider's network posture admit the proxy endpoint T-021 established?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Not until the egress requirement was corrected. denied emits no outbound rule, so even a loopback proxy port is unreachable, and host grants every host. The renderer now supports the localhost-port rule and the proxy ADR must add the posture that uses it |
| Evidence | \[evidence/T-021.md; adrs/2026-11-14-darwin-sandbox-provider/spec.toon (FR-012)\](evidence/T-021.md; adrs/2026-11-14-darwin-sandbox-provider/spec.toon (FR-012)) |

### T-051: Can the bwrap backend expose the T-014 proxy socket through its existing contributed-mount path?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | No. A contributed mount that holds a unix socket is refused because a read-only bind does not stop a connection, so the endpoint must be a backend-owned socket bind and a record entry added by the proxy ADR |
| Evidence | adrs/2026-11-07-linux-bwrap-provider/spec.toon (FR-006); evidence/T-014.md |

### T-052: Does a resident holder make a background command addressable after the shell that started it exits?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A child inherits the applied profile and stays in the holder's process group unless it calls setsid, so signalling the group reaches it after its own shell exits. T-045 proved inheritance and group signalling, and T-049 proved apply-before-exec in a forked child is safe |
| Evidence | \[evidence/T-045.md; evidence/T-049.md\](evidence/T-045.md; evidence/T-049.md) |

### T-053: Can a project supply Xcode itself as contributed read paths, so the backend needs no toolchain knowledge?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A compiler builds with the Xcode installation root and the licence plist as read mounts and needs no writable host path; cache warnings are non-fatal. The backend can drop Xcode awareness entirely |
| Evidence | [evidence/T-053.md](evidence/T-053.md) |

### T-054: Can SBPL name paths through sandbox_init_with_parameters instead of escaping them into the profile text?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. Paths pass as parameter values, so awkward names need no escaping and the apply ceiling rises above 2600 paths. Cost at the 64-path collapse matches inline, so the renderer should parameterise paths; tag handling is not covered |
| Evidence | [evidence/T-054.md](evidence/T-054.md) |

### T-055: Do the deny message, mach lookup and network positions accept parameters?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes. A parameter works in the deny message, global-name and remote ip, so the tag, mach services and loopback endpoints travel out of band and the renderer needs no escaper; only NUL and charset validation remain |
| Evidence | [evidence/T-055.md](evidence/T-055.md) |

## Decision

Adopt bubblewrap on Linux and App Sandbox via the sandbox_init entry point on macOS. The entitlement API requires a signed bundle, so unsigned devShell binaries use the deprecated entry point; its longevity is a carried risk. Egress beyond loopback needs a proxy on both platforms, and the two platforms do not share a transport: macOS reaches it as loopback TCP on a granted port (T-021), Linux through a bind-mounted unix socket outside the network namespace (T-014), so the network facet is one endpoint per platform rather than one shape. On Linux the sandbox is a holder process (the pause-container shape, as Flatpak enter does it): bwrap runs a sleeping child in the pid, net, mount and user namespaces, and every command is a setns join from outside. That restores shared pid and network context, gives pause and resume a sandbox-level meaning through the holder's cgroup, and closes the setsid escape, because killing pid 1 kills the whole pid namespace. T-013 is proven unprivileged on the same uid. T-015 is answered: the devShell builds on aarch64-darwin. T-019 is answered: joined commands are assigned to the sandbox scope before exec, so freeze and limits cover every command; without cgroups, pause is a stop-and-rescan fixpoint that a one-pass sweep does not implement and that a same-uid peer can livelock, so cgroups are the primitive and enumeration is the fallback. On macOS there is no namespace to hold open, but lifecycle still needs a process to hang a group on: one resident holder owns the sandbox session, every command is its child, and the profile is applied in each child. Commands already share the host's pids and network. Carried risks: a user manager without cgroup delegation makes the scope start fail and the caller selects the pid-namespace fallback, which still closes the setsid gap but cannot bound memory or task count; cgroup delegation is not a security boundary against a hostile same-uid workload. The destination is now proven on both platforms: the devShell environment is captured on the host by differential probe (T-011) and a devShell build runs inside the holder with no network and a read-only store (T-016). nix is deliberately unavailable inside the sandbox, because the daemon socket lets a workload write to the host store (T-018). On Linux egress is a host-side proxy reached over a bind-mounted unix socket, so the sandbox needs no route at all (T-014). The store view is the devShell's 238-path closure rather than the whole 57,534-path store, derived from Nix rather than discovered by failure, and the same test passes against it (T-022). T-017 is answered: a devShell build runs inside the macOS sandbox with the captured environment and needs no baseline addition, though the capture must be sanitised of build-sandbox scratch paths and NIX_BUILD_TOP must stay set for the clang wrapper. T-021 is answered: the macOS proxy is a loopback TCP listener on a granted port, because a unix socket is not reachable without AF_UNIX rules the profile does not emit. The two backends share only the sandbox surface, the resolved environment and an opaque proxy endpoint; their components differ and are drawn in architecture.mmd. Still unproven: a devShell whose closure exceeds the toolchain closure. Revised on the store view: bind the realised closure by default after validating a multi-user, root-owned Nix installation, with the whole store as an explicit option for workloads that reach undeclared paths; the closure covers declared dependencies only, so the wide bind is the fallback. Darwin production is charted and answered (T-043 to T-049). The mechanism works without Nix: a platform baseline plus the workspace confines a command and denies everything else, and a devShell is optional. It is not entitlement App Sandbox but the sandbox_init entry point, applied by a helper process that the backend execs, because the profile is per-process and the server must never confine itself; readiness is a symbol check plus the same helper applying a minimal profile. The helper is environment-transparent, so the backend owns withholding, rebasing and the HOME/TMPDIR/PWD triple exactly as the Linux backend does. Mounts cannot be rendered one rule per path: 238 grants cost about 43 ms per command and the serialised profile is rejected at 65535 bytes near 1010 grants, so the renderer collapses a contribution list to a common ancestor (for a devShell closure, /nix/store) rather than failing. Lifecycle is a resident holder: one supervisor per sandbox is the session and group leader, every command is its child, and pause, resume and delete signal the group, so a background server whose shell exited is covered; a setsid child still escapes. The backend knows no toolchain: a project that needs a compiler contributes the Xcode installation root and licence plist as read mounts (T-053), so there is no discovery, no writable host path and no toolchain key.

ADR: `adrs/2026-11-14-darwin-sandbox-provider`
