# T-011 nono gap inventory

nono 0.81.0, pinned exactly. Each entry is something the fixture needed that the
capability model could not express, or expressed badly. "Workaround" says how
far the prototype got; none of these is a fix.

| # | Gap | Observed | Workaround |
|---|---|---|---|
| 1 | `/dev/null` is not writable | Every `> /dev/null` in a shell script fails with EPERM. The first fixture run reported all checks as denied, which looks exactly like a working sandbox. | `platform_rule("(allow file-write* (literal \"/dev/null\"))")`. Raw SBPL. |
| 2 | No per-host network grant on macOS | `allow_tcp_connect` and `allow_tcp_bind` are documented Linux-only and return an error on macOS. Only `allow_localhost_port` exists, so a connect grant cannot be expressed. | `platform_rule("(allow network-outbound (remote ip \"host:port\"))")`. Raw SBPL, and it is the same rule the custom prototype emits directly. |
| 3 | Paths must exist | `allow_path` rejects anything that does not resolve, including `/private/var/db/dyld` on this host. The first two runs died in argument parsing, before the sandbox was applied. | Filter the baseline against the filesystem. The policy is a function of the host, not a constant. |
| 4 | `/private/var/select` is not in the default read set | Every shell prints `Error opening /private/var/select/sh: Operation not permitted` on startup. `/bin/sh` is a symlink into `/private/var/select`. | None. The shell still runs, but the message is emitted on every invocation. |
| 5 | cwd outside the grants breaks `getcwd` | Running from a directory that is not readable produces `getcwd: cannot access parent directories` and cargo fails with `couldn't get the current directory of the process`. | Start the process inside the workspace. Not a general fix: a caller cannot choose the cwd of an arbitrary command. |
| 6 | Resource limits are Linux-only | `with_resource_limits` carries memory and process ceilings, enforced by cgroup v2 on Linux. | None on macOS; recorded so the comparison does not overstate the library. |

## What the escape hatch costs

Gaps 1 and 2 were closed by passing SBPL through `platform_rule`, which is
validated only against root-level grants. At that point the caller is writing
the same policy the custom prototype writes, without the type checking, the
path canonicalisation, or the invariant denies. The abstraction is intact for
filesystem paths and leaks completely for the two things a sandbox for agents
most needs: a writable `/dev/null` and a specific egress target.

## Dependency surface

`cargo tree` for the prototype is 203 lines. The library pulls keyring,
sigstore-verify, x509-cert and sha2 even though this prototype uses none of them.
Cold build: 39s, against 0.85s for the custom binary with no dependencies.

| 7 | No deny-default tag | The library owns the profile, so `(deny default (with message ...))` cannot be set; `--tag` is accepted for CLI parity and warns. | None. Diagnostics fall back to pid attribution. |
| 8 | No credential invariants | The capability model has no equivalent of the credential deny block, so a caller grant over the home exposes credentials. | Raw `platform_rule` denies per credential path, appended after the capability model. |
