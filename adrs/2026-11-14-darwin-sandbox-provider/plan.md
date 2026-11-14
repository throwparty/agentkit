# Darwin Seatbelt Provider Plan

## Approach

Seatbelt is one backend module plus one helper binary. The helper is the prototype's `poc/custom`, promoted rather than rewritten: its renderer is kept, its argument surface and environment source change. The holder is one process per sandbox; it does not confine itself, it renders the profile once, and it forks a child per command that applies the profile with `sandbox_init_with_parameters` and execs. The server never applies a profile and never uses `pre_exec`. Every dynamic SBPL value is a parameter, so the renderer has no escaper. The profile is typed and layered, and the writer is the only place that formats. Mounts are validated, and a step with more than 64 paths is collapsed to its longest common ancestor. Configuration is machine-owned: defaults plus additions, with `baseline = 'none'` to opt out and harden. The decisions below are final; sequencing belongs to the tasks stage.

| ID | Decision | Choice |
|---|---|---|
| D1 | SBPL source | In-house renderer. No maintained typed crate exists; nono was rejected for dependencies and gaps. |
| D2 | Substitution | `sandbox_init_with_parameters` for every dynamic string: paths, tag, mach services, endpoints (T-054, T-055). |
| D3 | Profile model | Ordered typed layers and one writer. Ordering is structural, not a comment. |
| D4 | Configuration | Machine-owned section. `baseline = 'default'` adds to defaults; `baseline = 'none'` uses the lists exactly. Credential denies are always defaults plus additions. |
| D5 | Tag | The sandbox name, validated as `[A-Za-z0-9._-]{1,64}`, passed as a parameter. |
| D6 | Mach services | Parameters; defaults plus machine additions, or exactly the list when the baseline is off. |
| D7 | Network | Config is `denied` or `host`. The renderer also accepts loopback endpoints for the proxy ADR. Declared ports are refused. |
| D8 | Collapse | 64 paths per step; longest common ancestor; refuse a common ancestor of `/`. |
| D9 | Protocol | Versioned binary frames, one request per connection, 1 MiB frame cap. |
| D10 | Holder | One per sandbox, unconfined, `setsid`, poll loop, 64 concurrent commands. |
| D11 | Output | 1 MiB per stream, drain beyond, truncation flags, lossy UTF-8. |
| D12 | Record | Payload version 1, no environment values. |
| D13 | Readiness | `dlsym` the parameters symbol, trust the helper, probe it, check the state root. |
| D14 | Workspace | Copy into the state directory; transfers never follow a link. |
| D15 | Toolchain | No knowledge of Xcode or any compiler; a project contributes the read paths. |
| D16 | Limits and forwarding | Not implemented. Seatbelt has no quota, bind or accept. |
| D17 | Thresholds | Constants, not configuration: collapse 64, text 60 KiB, parameters 4096, frame 1 MiB, output 1 MiB, concurrent commands 64. |
| D18 | Socket path | A 16-random-byte hex name in the user temp directory, recorded, 0600. A state path can exceed the 104-byte socket limit. |
| D19 | Inspect | Paused comes from the record; an active holder is asked with Status and a one-second deadline. |
| D20 | Create rollback | Any failure after the state directory exists kills the holder and removes it. |
| D21 | Orphaned group | Delete signals the group when the holder pid is absent; a recycled pid is never signalled. Pause and resume signal only a live matching holder. |

## Architecture

## Module layout

```text
crates/agentkit-litterbox/
├── src/backends/seatbelt/
│   ├── mod.rs            BACKEND = 'seatbelt'; construct::build
│   ├── config.rs         SeatbeltConfig, Settings, defaults merge
│   ├── defaults.rs       BASELINE_READS, MACH_SERVICES, CREDENTIAL_DENIES
│   ├── host.rs           Host trait, SystemHost, FakeHost
│   ├── path.rs           expand, normalize, validate
│   ├── mounts.rs         validate contribution, collapse, ancestors
│   ├── environment.rs    merge, withhold, real home, HOME/TMPDIR/PWD
│   ├── policy.rs         ProfileInput, Profile, Rule, Filter, render
│   ├── protocol.rs       Frame, encode, decode, limits
│   ├── holder.rs         spawn, connect, config, exec, signal, status
│   ├── payload.rs        PayloadV1 and validation
│   ├── readiness.rs      symbol, helper, probe, state root
│   ├── workspace.rs      copy, upload, download
│   └── backend.rs        Backend and Lifecycle
├── src/bin/litterbox-seatbelt-helper/
│   ├── main.rs           the platform gate and the entry point
│   └── macos.rs          --probe and --hold; includes policy.rs and protocol.rs by #[path]
└── tests/seatbelt.rs     host suite
```

`policy.rs` and `protocol.rs` are dependency-free. The library compiles them for tests and the helper includes them by path. Neither names a `crate::` item, so the helper links only `libc` and `std`.

## Types

```rust
pub struct ProfileInput {
    pub workspace: PathBuf,
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    pub posture: Posture,
    pub tag: String,
    pub real_home: PathBuf,
    pub baseline_reads: Vec<PathBuf>,
    pub mach_services: Vec<String>,
    pub credential_denies: Vec<PathBuf>,
}

pub enum Posture {
    Denied,
    Host,
    Loopback(Vec<String>),
}

pub struct RenderedProfile {
    pub text: String,
    pub parameters: Vec<Parameter>,
}

pub struct Parameter {
    pub name: String,
    pub value: CString,
}

pub enum RenderError {
    TooLarge { bytes: usize },
    TooManyParameters { count: usize },
    BadPath { value: String },
    InvalidTag { value: String },
    InvalidService { value: String },
    InvalidEndpoint { value: String },
}
```

## Configuration

```toml
[backends.seatbelt]
network = 'denied'
baseline = 'default'
baseline-reads = []
mach-services = []
credential-denies = []
```

`SeatbeltConfig` uses `#[serde(deny_unknown_fields)]` and follows `DockerConfig`: `from_section` rejects `setup-command` with a pointer to the provision section, and `settings()` produces the resolved values.

`network` is `denied` (default) or `host`. `baseline` is `default` (default) or `none`.

With `baseline = 'default'`, `baseline-reads` and `mach-services` extend the defaults. With `baseline = 'none'`, the lists are the exact set and the defaults are not emitted. The structural rules stay in both modes: the root literal, process rules, metadata ancestors, workspace reads and writes, `/dev/null` and `sysctl-read`. A command in a `none` baseline sees only the workspace unless the lists say otherwise.

`credential-denies` is add-only in both modes. The defaults below always apply, because a config that can remove them turns the invariant into a suggestion.

Paths accept an absolute path or `~/`. A relative path is `Misconfigured`. `~/` expands with the real home from `getpwnam`, not `HOME`. Normalisation resolves `.` and `..` lexically. A configured path that fails `realpath` at create is `Misconfigured`, naming the key and the path. A default that is absent is skipped with a debug log. Duplicates keep the first occurrence.

Machine service names match `[A-Za-z0-9._@-]{1,255}`. The tag matches `[A-Za-z0-9._-]{1,64}`. Unknown keys are refused. A project file that sets any key is reported and ignored by the existing loader rule.

Defaults:

- Baseline reads: `/System/Library`, `/System/Applications`, `/Library/Frameworks`, `/usr/bin`, `/usr/lib`, `/usr/share`, `/usr/libexec`, `/bin`, `/sbin`, `/private/etc`, `/private/var/db/dyld`, `/private/var/select`, `/dev`.
- Mach services: the twelve names from the prototype policy, starting `com.apple.system.logger` and ending `com.apple.SystemConfiguration.configd`.
- Credential denies: the list in FR-013, rooted at the real home.

## Profile

The writer emits fixed layers in this order:

1. `(version 1)` and `(deny default (with message (param 'p0')))`.
1. `(allow process-exec)`, `(allow process-fork)`, `(allow process-info* (target same-sandbox))`, `(allow signal (target same-sandbox))`.
1. Reads: `(literal '/')`, baseline subpaths, workspace subpath, grant subpaths.
1. Metadata: one `(allow file-read-metadata ...)` with every ancestor of every read and denied path as `(literal (param 'pN'))`.
1. Writes: `(literal '/dev/null')` and the workspace subpath.
1. `(allow sysctl-read)`.
1. Mach lookup: one `(global-name (param 'pN'))` per service.
1. Network: nothing when denied; `(allow network-outbound)` when host; `(allow network-outbound (remote ip (param 'pN')))` per endpoint when loopback.
1. Credential denies last: `(deny file-read* ...)`, `(deny file-write* ...)`, `(deny file-write-unlink ...)`, each over the deny subpaths.

The root literal is always present and not configurable. T-043 measured that every exec dies with SIGABRT without it.

Every dynamic value is interned by exact bytes. The first use assigns `p0`, `p1` and so on; a repeated value reuses its name. The renderer never escapes a value, because no value is in the text.

The metadata ancestors are required. A global metadata rule lets a denied path be `stat`ed, and narrowing the rule to the grant subtree breaks resolution with ENOTDIR because the kernel stats each ancestor first.

The writer checks the text against 60 KiB and the parameter table against 4096 entries. Either overflow returns `TooLarge` or `TooManyParameters`, reported as status 71 and mapped to `Misconfigured`. No profile reaches the entry point after a guard fails.

## Wire protocol

Header, 12 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | magic `LBX1` (`4c 42 58 31`) |
| 4 | 2 | version, little endian, 1 |
| 6 | 1 | kind |
| 7 | 1 | flags, zero |
| 8 | 4 | payload length, little endian |

Kinds: 1 Config, 2 Ready, 3 Exec, 4 Stdout, 5 Stderr, 6 Exit, 7 Error, 8 Status, 9 StatusReply.

Config payload:

```text
u32 workspace_len      bytes
u16 read_count         [u32 len, bytes]*
u16 write_count        [u32 len, bytes]*
u8  posture            0 denied, 1 host, 2 loopback
if 2: u16 endpoint_count  [u32 len, bytes]*
u32 tag_len            bytes
u32 env_count          [u16 name_len, bytes, u32 value_len, bytes]*
u16 baseline_count     [u32 len, bytes]*
u16 mach_count         [u32 len, bytes]*
u16 deny_count         [u32 len, bytes]*
u32 home_len           bytes
```

Exec payload: `u16 argc` then `[u32 len, bytes]*`. Stdout and Stderr carry raw bytes, at most 64 KiB each. Exit is `i32 exit_code` and `u8 flags`, where bit 0 marks truncated stdout and bit 1 truncated stderr. Error is `u8 code` and `u32 message_len` plus bytes, with code 1 apply, 2 policy, 3 exec, 4 protocol, 5 internal, 6 busy. Status and StatusReply are empty.

Limits: frame at most 1 MiB, argc at most 4096, environment at most 4096 entries, each string at most 64 KiB. One request per connection. The holder rejects a peer whose uid is not its own, read with `getpeereid`.

## Holder

Startup for `--hold <socket>`:

1. `setsid()`. A failure exits 70 and create fails.
1. `umask(0o077)`.
1. Bind the path create supplied, refusing a symlink, and `chmod(0600)`.
1. `holder.log` is opened `0600` and truncating for stderr; stdin and stdout are null.
1. Accept one Config connection, render the profile, reply Ready, keep the configuration.
1. Enter the poll loop.

`--probe` applies `(version 1)(deny default)` with an empty parameter array and exits 0, or 70 with the message. This is the readiness probe.

The poll loop is single-threaded and never blocks on a child. The listener and connection fds are non-blocking. Connection states are Reading, Running and Writing. At most 128 connections and 64 running children; beyond that, Error 6.

For Exec the holder forks. The child closes the listener and every connection fd, connects the two pipes to stdout and stderr, creates a close-on-exec status pipe on fd 3, chdirs to the workspace, calls `sandbox_init_with_parameters`, then `execv`. On apply failure it writes code 1 and the message to the status pipe and exits 70. On exec failure it writes code 3 and the errno text and exits 126. The parent closes the write ends and registers the read fds. A zero-length read of the status pipe at exit means the command ran.

Output is buffered per stream up to 1 MiB and drained beyond that, setting the truncation flag. The holder never stops reading, so a verbose child does not block. On child exit the holder waits, sends Exit, and closes. If the client disconnects before Exit, the holder kills the child pid; grandchildren stay in the holder's group and delete removes them. `SIGPIPE` is ignored.

## Backend

Create:

1. Validate the tag and resolve the real home from `getpwnam`; either failure is `Misconfigured`.
1. Merge contributions with `Contributions::environment_for(workspace)`, withhold values naming an absolute path outside the workspace and the grants, then set `HOME`, `TMPDIR`, `PWD`. `PATH` is only what the steps contributed.
1. Validate mounts and collapse a step above 64 paths to its longest common ancestor. Refuse an ancestor of `/` and a collapsed set still above 64.
1. Build the state directory `data_dir/litterbox/seatbelt/<project>/<name>/` with `0700` and copy the staged workspace into `workspace/`.
1. Generate a socket path in the user temp directory from 16 random bytes, mode 0600. Spawn the helper with a cleared environment, null stdin and stdout, stderr to `holder.log`, and `--hold <socket>`. Any failure after the state directory exists kills the holder and removes the directory.
1. Poll `connect` for up to 5 seconds. A timeout kills the helper and returns `Unavailable` with the tail of the log.
1. Send Config, read Ready or Error.
1. Run sandbox steps through the shared runner.
1. Write PayloadV1 and return the record.

A duplicate name is `Conflict` before any process starts. Declared ports are `Unsupported`, naming `network-bind`.

Exec reads the payload, validates the holder pid, start time and non-zombie state, and refuses with Denied while the record says paused. It then connects, sends Exec, and reads frames. Stdout and stderr append up to the shared `EXEC_OUTPUT_CAP`; Exit ends the exchange. A connect failure is `NotFound`; a start-time mismatch is `Denied`.

Pause and resume validate, then `kill(-pid, SIGSTOP)` or `SIGCONT`, set or clear `paused`, and rewrite the payload. Delete validates as far as possible. When the holder matches the record it sends `SIGKILL` to the group; when the holder pid is absent but the state directory exists it signals the recorded group and logs it as orphaned; a recycled pid with a different start time is not signalled. It waits up to 2 seconds, then removes the socket, log, workspace and state directory. A missing holder is success.

Inspect reports Paused from the record without asking a stopped holder. When the record is active it sends Status with a one-second deadline; a live holder that does not answer is `Error`, not `Active`. List rebuilds each payload under the state root for both scopes and reports a corrupt entry instead of dropping it. Upload and download copy through descriptors opened without following links and under the shared upload and download timeouts. Create logs the helper argv at debug level with every environment value left out. Capabilities report pause and resume available, limits and forwarding unsupported, and both contribution sorts available.

## Readiness

Each check carries state, reason and remedy:

1. macOS, satisfied at compile time.
1. `dlsym(RTLD_DEFAULT, 'sandbox_init_with_parameters')` is non-null.
1. The helper beside the current executable is a regular file after resolution, owned by root or the effective uid, and not group or world writable.
1. The probe helper exits 0 within 2 seconds.
1. The parent of the state root is writable.

Config failures are `Misconfigured`, not readiness. Create refuses on any check that is not available.

## Record

```rust
pub struct PayloadV1 {
    pub version: u32,
    pub helper: PathBuf,
    pub holder_pid: u32,
    pub holder_start_secs: i64,
    pub holder_start_micros: u32,
    pub socket: PathBuf,
    pub workspace_host: PathBuf,
    pub workspace_sandbox: PathBuf,
    pub reads: Vec<PathBuf>,
    pub collapsed: Vec<CollapsedStep>,
    pub provenance: Provenance,
    pub posture: String,
    pub tag: String,
    pub paused: bool,
}
```

`CollapsedStep` holds the step identity, the contributed count and the granted ancestor. The file is `0600` in a `0700` directory. A higher version is `Unavailable` with the remedy to delete and recreate; a lower or incomplete one is the same.

## Error mapping

| Condition | Error |
|---|---|
| Helper 1 apply | `Failed`, message `sandbox_init_with_parameters: ...` |
| Helper 2 policy or size | `Misconfigured` |
| Helper 3 exec | `Failed`, naming the command |
| Helper 4, 5 | `Failed` |
| Helper 6 busy | `Failed`, too many concurrent commands |
| Connect refused or socket absent | `NotFound` at exec, `Error` at inspect |
| Holder gone or zombie | `NotFound` at exec, `Missing` at inspect |
| Start-time mismatch | `Denied` |
| Payload version or field | `Unavailable`, delete and recreate |
| Declared ports | `Unsupported`, names `network-bind` |
| Config key, path, service, tag | `Misconfigured`, names the key |

## Technologies

| Technology | Role |
|------------|------|
| Rust | Edition 2024 and the standard library for paths, processes, maps and threads. |
| libc | fork, setsid, kill, waitpid, dlsym, getpeereid, getpwnam, realpath and proc_pidinfo. sandbox_init_with_parameters is declared extern. |
| Seatbelt | The macOS kernel sandbox. The helper applies a deny-default SBPL profile from parameters and execs, so every descendant inherits confinement. |
| Unix domain socket | The control channel at a recorded path in the user temp directory, mode 0600, carrying configuration, argv, output and status. |
| tokio | The existing runtime. Holder spawn, connect and response reads run on blocking threads. |
| serde and toml | The versioned record payload, refused when the version or a required field is wrong. |
| arch-lint | A scope for the backend module with deny edges to the orchestrator, CLI, MCP, persistence, SCM and provision preparation. |
| cargo test | Unit tests beside each module, golden profile tests, protocol codec tests and one host suite. |

## Components

### config.rs

Declare and resolve the machine-owned section

SeatbeltConfig with deny_unknown_fields: network (denied default, host), baseline (default or none), baseline-reads, mach-services, credential-denies. from_section rejects setup-command with a pointer to \[[provision]\]. settings returns Settings with network, baseline mode, merged read and service lists, and deny additions. Each key failure is Misconfigured and names the key.

### defaults.rs

Hold the three default tables as data

BASELINE_READS, MACH_SERVICES and CREDENTIAL_DENIES as &'static [&'static str]. The baseline excludes every developer path and /nix. The credential list is the FR-013 set, rooted at the real home at render time. One test asserts each table is non-empty and free of duplicates.

### path.rs

Turn configuration strings into absolute paths

expand replaces a leading ~/ with the real home from getpwnam. normalize removes . and resolves .. lexically. validate rejects a relative path, a NUL byte and an empty string. A default that fails realpath at create is skipped with a debug log; a configured path that fails is Misconfigured naming the key and value.

### host.rs

Make every host read injectable

Host declares pid_exists, pid_state, pid_start_time, pid_group, signal_group, connect_socket, peer_uid, real_home, realpath, clock and proc_pidinfo. SystemHost answers from libc, sysctl and the file system. FakeHost answers from a table. The backend holds Arc<dyn Host> and opens no host resource directly.

### mounts.rs

Validate contributions and collapse them

validate rejects a path that is not absolute, not resolvable, a socket, device or FIFO, the workspace or an ancestor of it, a protected root, the metadata directory, the state directory, or a credential path. collapse replaces a step above 64 paths with the longest common ancestor by component comparison, deduped. It refuses a common ancestor of / and a collapsed set above 64, naming the step and count. ancestors returns the parent chain used by the metadata layer.

### environment.rs

Merge, withhold and fix the three owned variables

merge folds steps through Contributions::environment_for(workspace), withholds a variable whose value has an absolute element outside the workspace and the grants (split on :), creates workspace/tmp, then sets HOME=workspace, TMPDIR=workspace/tmp and PWD=workspace. PATH is only contributed. The real home is resolved through Host, not from HOME. Provenance records Set for the three and Removed for withheld names. Values are returned as a map and sent only over the socket.

### policy.rs

Render the profile and its parameter table

The only module that emits SBPL. It builds ordered layers from ProfileInput, resolves every path with realpath and returns BadPath if one fails, interns every dynamic value by exact bytes as p0..pN, emits the fixed text described in the architecture, computes metadata ancestors, and enforces the 60 KiB and 4096 parameter guards. It has no escaping function and no dependency on the crate. Golden tests assert the text with parameter names and the full parameter table.

### protocol.rs

Encode and decode frames

Frame with magic LBX1, version 1, kind, flags and payload length. encode and decode for every kind, with bounds checks before allocation. Limits: 1 MiB frame, 4096 argv entries, 4096 environment entries, 64 KiB per string. A short header, bad magic, bad version or over-length frame is a protocol error. The module has no dependency on the crate.

### seatbelt-helper

Hold the sandbox and confine each child

Second binary target. --probe applies a minimal profile and exits. --hold setsid, binds the 0600 socket, logs to holder.log, accepts Config, renders through policy.rs, replies Ready, then runs the poll loop. For Exec it forks, the child applies the profile with sandbox_init_with_parameters and execs, and the parent streams output and returns status. Exit codes 70, 71 and 126 keep apply, policy and exec failures distinct. It links only libc and std.

### holder.rs

Address the holder from the backend

spawn uses the socket path create generated, starts the helper with a cleared environment and stderr to holder.log, polls connect for 5 seconds, sends Config and waits for Ready. exec connects, sends Exec and decodes frames into Execution, truncating at EXEC_OUTPUT_CAP. pause, resume and destroy signal the recorded group after checking pid, start time and zombie state. status sends Status for inspect. The socket path is the one create generated in the user temp directory.

### payload.rs

Persist what a later process needs

PayloadV1 with the fields in the architecture, toml through the metadata store. from_record refuses a higher version or a missing field as Unavailable with the delete-and-recreate remedy. write asserts 0600 on the file and 0700 on the directory. validate checks pid existence, start time, zombie state and socket path shape.

### readiness.rs

Report whether the mechanism can confine here

Five checks: macOS, dlsym of sandbox_init_with_parameters, helper trust, probe exit, state root parent writable. Each returns a report Check with state, reason and remedy. No check confines the caller or starts a holder. create refuses on any unavailable check.

### workspace.rs

Materialise and transfer the workspace

copy_from copies the staged directory into workspace/ with each component opened without following a link, refusing a symlink, device or FIFO. transfer_in and transfer_out resolve beneath an open directory descriptor. download copies recursively, preserves modes and keeps a symlink as a symlink. Every path given to the holder is built from the state root and the sandbox name.

### backend.rs

Implement the contract and the lifecycle facet

Backend: create, list, exec, upload, download, inspect, delete, readiness, capabilities and contribution_support with environment and mounts available. Lifecycle: pause and resume. No Limits and no Forwarding. create refuses ports as Unsupported and duplicates as Conflict, and returns only after Ready. inspect reports posture, workspace and provenance. list rebuilds payloads for both scopes.

### tests

Cover the refusals as well as the happy path

Unit: config defaults and merge, baseline none, add-only denies, unknown keys, path validation, collapse boundaries, metadata ancestors, parameter interning, golden profiles for both postures, guards, protocol round trip and malformed frames, payload versions, readiness states and error mapping. Host: create with no steps, exact environment, write outside denied, credential read denied, descendant confined, 238-path collapse, pause and resume, background command, delete, setsid escape, holder killed out of band, concurrent commands, status probe. Ignored: a C compile with contributed Xcode paths.

### registry and docs

Make it selectable and keep the rules honest

Add the registry entry under cfg(target_os = 'macos') with lifecycle only. Extend docs/docs/user/litterbox/backends.mdx with readiness, postures, refused ports, no limits, no PATH without steps, baseline modes, and no toolchain knowledge. Extend reference/config.mdx with the five keys. Add an arch-lint scope and a negative fixture for each deny edge.

## Data Flow

Create resolves the sequence and prepares host steps before the branch exists, so the request arrives with contributions. The backend merges the environment, withholds values naming ungranted paths, validates and collapses mounts, resolves the real home, and creates the state directory and workspace. It spawns the helper with setsid, waits for the socket, sends Config with the resolved lists, environment and tag, and receives Ready. The helper renders the profile once and keeps it. The backend writes PayloadV1 and returns.

A later exec reads the payload, checks the holder pid and start time, connects, sends argv, and receives stdout, stderr and exit frames. The holder forks, the child applies the profile and execs with the configured environment and workspace, and the holder drains both streams under the cap and returns the status.

Pause and resume signal the holder's process group, so a command started earlier and still resident stops and continues with the sandbox. Delete kills the group, removes the socket, log, workspace and state directory. Inspect reports Paused from the record; when active it sends Status to prove the socket before reporting Active.

## Deployment

There is no daemon and no service. One unconfined holder exists per sandbox; the confinement boundary is the child it forks. The backend compiles on macOS only and the registry does not list it elsewhere. State lives at data_dir/litterbox/seatbelt/<project>/<sandbox>/, holding the workspace and holder.log, with 0700 directories and 0600 files; the socket is at a recorded path in the user temp directory. The helper is a second binary in agentkit-litterbox, installed beside the main binary; readiness refuses a missing or writable helper. A machine that selects nothing keeps Docker. No macOS test job exists; the host suite runs on a developer machine or a dedicated runner and, when a job is added, selects only -p agentkit-litterbox --lib backends::seatbelt and --test seatbelt, treats unavailable as failure, and reuses nix develop ./nix.
