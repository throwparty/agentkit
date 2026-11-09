# Bubblewrap Network Setting

**Status:** draft  **Created:** 2026-11-09  **Author:** adrian

Terms used in this document.

**Sandbox**: the isolated environment Litterbox creates with bubblewrap. It is seven Linux namespaces plus one long-lived process that keeps them alive.

**bubblewrap (bwrap)**: the program Litterbox runs to create the sandbox.

**Namespace**: a Linux isolation boundary. The sandbox has seven: user, mount, pid, network, IPC, UTS and cgroup. A process in a namespace sees only what that namespace exposes.

**Network namespace**: the namespace that decides which network interfaces and ports a process sees. A separate network namespace gives a process its own loopback and nothing else.

**`network` setting**: `[backends.bubblewrap] network` in Litterbox's configuration. It accepts `denied` or `host`. The default is `denied`.

**`denied`**: the setting value that gives the sandbox its own network namespace. The sandbox can reach only its own loopback, and the host cannot connect to a port inside it.

**`host`**: the setting value that does not create a network namespace. The sandbox uses the host's network, so it can reach the internet and the host can connect to a port it listens on.

**User configuration**: `~/.config/agentkit/litterbox/config.toml`. The person running Litterbox owns this file.

**Project file**: `.agentkit/litterbox/config.toml` in the repository. It is committed. It cannot choose `network`.

**Local override**: `.agentkit/litterbox/config.local.toml`. It is untracked. It cannot set keys that belong to the person running Litterbox, such as `network`.

**`payload.toml`**: the file Litterbox writes when it creates a sandbox, in the sandbox's state directory. It holds the process ids, the seven namespace numbers, the mounts, the `network` value and the port forwards. A later `litterbox shell` reads it.

**State directory**: `~/.local/state/agentkit/litterbox/bubblewrap/<project>/<sandbox>/`. It holds the workspace copy and `payload.toml`.

**Namespace number**: the number shown by `readlink /proc/<pid>/ns/<name>`, for example `net:[4026531840]`. Litterbox saves the sandbox's seven numbers in `payload.toml`.

**MCP `sandbox-create`**: the tool an agent calls to create a sandbox. It returns the inspection JSON.

**Inspection JSON**: the object returned by `sandbox-create`. It has `name`, `backend`, `status`, `egress`, `workspace`, `environment` and `forwards` fields.

**`egress`**: the inspection JSON field that says whether the sandbox has its own network namespace or the host's. Its values are `denied` and `direct`.

**`direct`**: the `egress` value for a `host` sandbox.

**`denied` (egress)**: the `egress` value for a `denied` sandbox. It is the same word as the setting value; the field is named `egress`.

**`Forward`**: a port the sandbox listens on and the host can connect to. It has `name`, `target`, `host_port`, `bind` and `env_var` fields.

**`litterbox shell`**: the command that runs a command in an existing sandbox, for example `litterbox shell demo -- uname -s`.

**Mount**: a host path made visible inside the sandbox at the same path.

**EPERM**: the kernel's permission error. Litterbox sees it when it tries to enter the host's network namespace as a normal user.

Background. The `network` setting is already in the code under `crates/agentkit-litterbox/src/backends/bubblewrap/` and in the docs. This ADR fixes what is still wrong. The code that runs a command in an existing sandbox can skip more than the network namespace. The inspection JSON says `denied` for a dead sandbox even when `payload.toml` says `host`. A port can be silently dropped. There is no real-sandbox test for `host`. The ADR also records the DNS and TLS mounts that make `host` usable.


## Problem

With `network = 'host'`, the code that runs a command in an existing sandbox skips any namespace whose number matches the caller's. That is right for the network namespace, because entering the host's network namespace fails with EPERM, but it could also skip another namespace and leave the command partly outside the sandbox. The inspection JSON also hardcodes `denied` for a sandbox whose process is gone, even when `payload.toml` says `host`. A port whose name cannot become an environment variable can vanish from the list. And there is no test that creates a real `host` sandbox and proves the host can reach a port inside it.

## Goals

- A person can set `network = 'denied'` or `network = 'host'` in `~/.config/agentkit/litterbox/config.toml`. The default is `denied`.
- A project file cannot choose `host`. If `.agentkit/litterbox/config.toml` sets it, Litterbox prints a notice and uses the user's value or the default. A `config.local.toml` that sets it is an error.
- When `litterbox shell` runs a command in an existing sandbox, it enters the sandbox's other six namespaces. It skips the network namespace only when `payload.toml` says `network = 'host'`. It then checks all seven namespace numbers against `payload.toml`.
- The `egress` field in the inspection JSON says `direct` when `payload.toml` says `host` and `denied` when it says `denied`, including when the sandbox process is gone. If `payload.toml` cannot be read, inspection returns an error.
- With `network = 'host'`, a project port is reported as a `Forward` the host can connect to, with the same port number. With `network = 'denied'`, create fails and names the setting. A bad port name is an error, not a dropped port.
- With `network = 'host'`, the sandbox sees the host's `/etc/resolv.conf`, `/etc/hosts`, `/etc/nsswitch.conf` and `/etc/ssl` as mounts, and `SSL_CERT_FILE` is set, so DNS and TLS work.
- A real `host` sandbox is created and checked: its network namespace number equals the host's, the other six differ, a listener inside is reachable from the host, and a `denied` sandbox still refuses the connection.
- Tests cover all of this, and `cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` pass.

## Non-goals

- A proxy or filtering by destination. `host` means the host's network.
- Changing Docker.
- Reading old v1 `payload.toml` files. That is a separate release task.
- Pause, resume, delete, limits, or the holder's lifetime.
- Deleting stale state directories or branches.


## Functional Requirements

### FR-001: The setting

`[backends.bubblewrap] network` accepts `denied` or `host`. If it is missing, it is `denied`. Any other value, or any other key in that table, is a configuration error. A project file that sets `network` is ignored and reported with a notice; the user configuration or default decides. A local override that sets `network` is rejected, the error names the key, and it says to put the setting in the user configuration. The backends page and the configuration reference describe both values, the default and the rule. Tests cover `denied`, `host`, the default, an unknown value, an unknown key, the project-file notice and the local-override error.

**Slug:** `network-setting`

### FR-002: DNS and TLS under host

With `network = 'host'`, the arguments Litterbox gives bubblewrap include read-only mounts for `/etc/resolv.conf`, `/etc/hosts`, `/etc/nsswitch.conf` and `/etc/ssl` when those paths exist, and the sandbox environment sets `SSL_CERT_FILE` to the host's CA bundle. With `network = 'denied'`, none of those mounts or variables are added. A test checks the arguments for both settings.

**Slug:** `host-dns-and-tls`

### FR-003: Running a command in a sandbox

`litterbox shell` runs a command in an existing sandbox by entering the sandbox's namespaces. With `network = 'denied'`, it enters all seven. With `network = 'host'`, it enters six and skips only the network namespace, because entering the host's network namespace fails with EPERM. It must not skip any other namespace. After entering, it compares the seven namespace numbers in `/proc/self/ns/*` with the seven numbers saved in `payload.toml`. If any differ, it fails and does not run the command. A unit test checks the skip decision for both settings. A real `host` sandbox checks that the command's network namespace number equals the host's.

**Slug:** `run-command-namespaces`

### FR-004: The egress field

The inspection JSON that `sandbox-create` returns has an `egress` field. When `payload.toml` can be read, `egress` is `direct` if `network` is `host` and `denied` if `network` is `denied`. This is true whether the sandbox process is alive or gone. If `payload.toml` cannot be read, inspection returns the existing payload-version or unreadable-file error and does not return an `egress` value. Tests cover live host, live denied, dead host, dead denied and unreadable.

**Slug:** `inspection-egress`

### FR-005: Ports

With `network = 'host'`, each `[[ports]]` entry becomes a `Forward` whose `host_port` equals `target`, with the project's `forwarding.bind` and the `LITTERBOX_FWD_PORT_*` environment variable in the sandbox. `payload.toml` records the forwards and the inspection JSON returns them. With `network = 'denied'`, create fails with `Unsupported` and names `network = 'host'`. A port name that cannot become an environment variable is an error in both the configuration check and the backend, not a dropped entry. Tests cover host, denied and a bad name.

**Slug:** `host-port-forwarding`

### FR-006: Real sandbox proof

An ignored test creates a real sandbox with `network = 'host'` and a declared port. It reads `/proc/<sandbox-pid>/ns/net` and `/proc/self/ns/net` on the host and checks they are equal. It runs `readlink /proc/self/ns/net` inside the sandbox and checks the command sees the same number. It checks the other six saved numbers differ from the host's. It starts `nc -l 127.0.0.1 <port>` inside the sandbox, connects from the host, and checks the listener receives the connection. It checks the inspection JSON says `direct` and contains the `Forward`. A second ignored test does the same with `network = 'denied'` and checks the host connection is refused. The commands and observed numbers are written to `adrs/2026-11-09-bwrap-network-completion/evidence.md`.

**Slug:** `host-network-proof`

### FR-007: Tests and lints

`cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` pass. The ignored real-sandbox tests run on this host with `cargo test -p agentkit-litterbox -- --ignored`, and their output is recorded.

**Slug:** `tests-and-lints`

## Non-functional Requirements

### NFR-001: A project cannot widen access

No project file and no local override can select `host`. If the user configuration does not say `host`, the sandbox is `denied`.

**Slug:** `project-cannot-widen`

### NFR-002: No extra machinery

The setting adds no proxy, forwarder, socket, daemon or extra process. `host` uses the host's network namespace; `denied` uses a separate one. The only additions to a `host` sandbox are the four read-only mounts and `SSL_CERT_FILE`.

**Slug:** `no-extra-network-machinery`

### NFR-003: No lifecycle changes

Pause, resume, delete, limits and the holder's lifetime are unchanged.

**Slug:** `no-lifecycle-change`

### NFR-004: Docs

The backends page and the configuration reference describe both values, the default, the project-file rule and the port behaviour.

**Slug:** `documentation`

### NFR-005: Tests

Unit tests cover the setting, the arguments Litterbox gives bubblewrap, and the skip decision. The ignored tests cover the real `host` and `denied` sandboxes.

**Slug:** `network-tests`

## Acceptance Criteria

### AC-001: The setting

A user configuration with `network = 'host'` creates a host sandbox. A project file that sets it prints a notice and the sandbox is denied. A local override that sets it is an error naming the key.

**Slug:** `setting-works`

### AC-002: DNS and TLS

A host sandbox has `/etc/resolv.conf`, `/etc/hosts`, `/etc/nsswitch.conf` and `/etc/ssl` mounted, and `SSL_CERT_FILE` set. A denied sandbox has none of them.

**Slug:** `host-dns-and-tls-works`

### AC-003: Namespaces

In a real host sandbox, `readlink /proc/self/ns/net` inside the sandbox equals the host's number and the saved network number. The other six saved numbers differ from the host's. A denied sandbox's network number differs from the host's.

**Slug:** `namespaces-work`

### AC-004: Egress field

The inspection JSON says `direct` for a live host sandbox and for a dead host sandbox whose `payload.toml` is readable, `denied` for a live or dead denied sandbox, and returns an error when `payload.toml` is unreadable.

**Slug:** `egress-field-works`

### AC-005: Ports

A host sandbox with a declared port reports a `Forward` with `host_port == target` and sets the environment variable. A denied sandbox with a port fails with `Unsupported` naming `network = 'host'`. A bad port name is an error.

**Slug:** `ports-work`

### AC-006: Real sandbox proof

A listener started inside a host sandbox is reachable from the host on the declared port. A listener inside a denied sandbox is not. The observed output is in `evidence.md`.

**Slug:** `host-listener-reachable`

### AC-007: Tests and lints

`cargo test` and `cargo clippy` pass. The ignored real-sandbox tests run and their output is recorded.

**Slug:** `suite-and-lints`

## Edge Cases

### EC-001: Project sets host

A committed project file with `[backends.bubblewrap] network = 'host'` is ignored with a notice. The sandbox is denied unless the user configuration says host.

**Slug:** `project-sets-host`
### EC-002: Local override sets host

A local override with `network` is rejected by name, and the error points at the user configuration.

**Slug:** `local-override-sets-host`
### EC-003: Host network namespace cannot be entered

Entering the host's network namespace returns EPERM. Litterbox skips only that one namespace and checks the saved network number instead.

**Slug:** `host-net-eperm`
### EC-004: Bad port name

A name that cannot become `LITTERBOX_FWD_PORT_*` is an error at configuration and at create. It is never dropped.

**Slug:** `bad-port-name`
### EC-005: Denied with ports

Creating a denied sandbox with ports fails before any sandbox starts and leaves no state directory.

**Slug:** `denied-with-ports`

