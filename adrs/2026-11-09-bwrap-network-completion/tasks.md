# Bubblewrap Network Setting Tasks

## Tasks

### T-001: [x] Test the setting and its ownership

Add tests in `config.rs` and `config_loader.rs` for both values and the ownership rule. `network = 'denied'` and `network = 'host'` parse. A missing value is `denied`. An unknown value and an unknown key are configuration errors. A project file that sets `network` is ignored with a notice and the user value or default decides. A local override that sets `network` is rejected and the error names the key. A user configuration with `host` reaches `BubblewrapSettings { network: Host }`.


| Field | Value |
|-------|-------|
| Success Criteria | The tests assert the parsed value, the default, the notice text and the error text. `cargo test -p agentkit-litterbox config` passes. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On |  |
| References | network-setting,setting-works,project-sets-host,local-override-sets-host,project-cannot-widen |

### T-002: [x] Test the arguments and host mounts

In `view.rs`, add tests for the arguments Litterbox gives bubblewrap under each setting. `denied` includes `--unshare-net`; `host` does not and still unshares the other five namespaces. Under `host`, the view requests `/etc/resolv.conf`, `/etc/hosts`, `/etc/nsswitch.conf` and `/etc/ssl` when those paths exist. Under `denied`, it requests none of them. If `build_view` is awkward to test because it reads the filesystem, extract the path list into a helper and test the helper.


| Field | Value |
|-------|-------|
| Success Criteria | The `denied` golden vector is unchanged. The `host` vector has no `--unshare-net`. The host path list contains the four paths and the denied list is empty. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On |  |
| References | host-dns-and-tls,host-dns-and-tls-works,network-tests |

### T-003: [x] Report egress from the payload for dead sandboxes

In `BubblewrapBackend::inspect`, use `data.network` in the `Verdict::Absent` branch as well as the live branch. The absent branch currently hardcodes `EgressPosture::Denied`. Keep the `Verdict::Editions` error for an unreadable payload. Add tests with a fake host for live host, live denied, absent host, absent denied and unreadable.


| Field | Value |
|-------|-------|
| Success Criteria | An absent host record reports `Direct`. An absent denied record reports `Denied`. An unreadable payload returns the edition error and no egress value. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | inspection-egress,egress-field-works |

### T-004: [x] Make the forward construction fallible

Extract the `Host` forward loop from `create_sandbox` into a function returning `Result<Vec<Forward>, Error>`. Call `forwarded_port_env_var` and return its error. Remove the `filter_map`. Keep the `denied` plus ports `Unsupported` error and its remedy. Add tests for a host port, a denied port and a bad port name.


| Field | Value |
|-------|-------|
| Success Criteria | No `filter_map` remains in the forward path. A host port produces a `Forward` with `host_port == target`. A denied port is `Unsupported` and names `network = 'host'`. A bad name is an error naming the port. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On |  |
| References | host-port-forwarding,ports-work,bad-port-name,denied-with-ports |

### T-005: Skip only the host network namespace

In `join.rs`, replace the `skip[7]` array computed by comparing `/proc/<sandbox-pid>/ns/<name>` with `/proc/self/ns/<name>` with a decision from `data.network`. When it is `Host`, skip the `setns` for `CLONE_NEWNET` only. When it is `Denied`, skip nothing. Enter the other six namespaces in both cases. Keep `namespaces_match` comparing all seven recorded numbers. Add a unit test for the decision.


| Field | Value |
|-------|-------|
| Success Criteria | The unit test shows `Denied` skips none and `Host` skips only `net`. No code reads `/proc/self/ns` before the fork. The real host test in T-007 sees the host's network number. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On |  |
| References | run-command-namespaces,namespaces-work,host-net-eperm,no-extra-network-machinery |

### T-006: Test the payload round-trip for network and forwards

Add a test in `payload.rs` that a `BubblewrapData` with `Network::Host` and a `Forward` round-trips through `into_value` and `from_record`. Do not change `VERSION` and do not add v1 handling.


| Field | Value |
|-------|-------|
| Success Criteria | The round trip preserves `network` and `forwards`. A v1 payload still fails as before. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On |  |
| References | inspection-egress,host-port-forwarding |

### T-007: Prove host and denied on real sandboxes

Add two `#[ignore]` tests in `backend.rs`. The host test constructs `BubblewrapSettings { network: Network::Host, .. }` and creates a sandbox with a `RequestedPort`. It checks that `/proc/<sandbox-pid>/ns/net` equals `/proc/self/ns/net`; that `readlink /proc/self/ns/net` inside the sandbox equals both; that the other six saved numbers differ from the host's; that `nc -l 127.0.0.1 <port>` inside is reachable from the host; and that `inspect` says `Direct` and returns the forward. The denied test checks that the network number differs from the host's and that a host connection is refused. Run both with `cargo test -p agentkit-litterbox -- --ignored` and write the commands, namespace numbers and listener output to `adrs/2026-11-09-bwrap-network-completion/evidence.md`.


| Field | Value |
|-------|-------|
| Success Criteria | Both ignored tests pass on this host. `evidence.md` contains the observed namespace numbers and listener output. The host test's listener receives the host connection. The denied test's host connection fails. |
| Complexity | 🔴 High |
| Effort | 5-8h |
| Depends On | T-002,T-003,T-004,T-005,T-006 |
| References | host-network-proof,host-listener-reachable,namespaces-work,suite-and-lints |

### T-008: Align the provider spec and docs

Amend FR-003 in `adrs/2026-11-07-linux-bwrap-provider/spec.md` and `spec.toon`. It should say the command enters every namespace except the network namespace when `payload.toml` says `host`, because that namespace is owned by the initial user namespace and `setns` fails with EPERM. It should also say the network number is still checked. Check `docs/docs/user/litterbox/backends.mdx` and `docs/docs/user/litterbox/reference/config.mdx` against the code. Do not touch lifecycle or holder sections.


| Field | Value |
|-------|-------|
| Success Criteria | The provider `spec.md` and `spec.toon` agree. `adr.build.sh` for the provider directory passes. The docs match the code. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-005,T-007 |
| References | documentation,run-command-namespaces,host-net-eperm,no-lifecycle-change |

### T-009: Run the suite and clippy

Run `cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`. Fix failures caused by these changes. Run the ignored tests and record their output.


| Field | Value |
|-------|-------|
| Success Criteria | Both commands pass. The ignored tests pass. No warnings remain. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-001,T-002,T-003,T-004,T-005,T-006,T-007,T-008 |
| References | tests-and-lints,suite-and-lints |

