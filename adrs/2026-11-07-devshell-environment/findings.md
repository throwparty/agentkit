# Findings: devshell-environment prototype

Host: Linux, Nix 2.34.9, bubblewrap 0.12.0. The harness is at `poc/prepare/`
(`devshell-poc`), excluded from the workspace by the root `Cargo.toml`'s
`exclude = ["adrs"]` and declared standalone in its own manifest. The declared
step is `poc/devshell.toml`.

## Reproduce

```bash
# The unit tests need no nix and no network.
cd adrs/2026-11-07-devshell-environment/poc/prepare
cargo test --offline

# bubblewrap is not in the devShell, so name its store path.
cd -
bwrap=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap)

# --out must not exist: the harness creates it exclusively and refuses to reuse
# a directory it did not create.
rm -rf /tmp/devshell-poc /tmp/devshell-poc-whole

./adrs/2026-11-07-devshell-environment/poc/prepare/target/debug/devshell-poc run \
    --step adrs/2026-11-07-devshell-environment/poc/devshell.toml \
    --project . \
    --out /tmp/devshell-poc \
    --workspace "$PWD" \
    --allow-project-setup \
    --bwrap "$bwrap/bin/bwrap" \
    -- cargo test -p agentkit-common

# The whole-store view, for comparison.
./adrs/2026-11-07-devshell-environment/poc/prepare/target/debug/devshell-poc run \
    --whole --step adrs/2026-11-07-devshell-environment/poc/devshell.toml \
    --project . --out /tmp/devshell-poc-whole --workspace "$PWD" \
    --allow-project-setup --bwrap "$bwrap/bin/bwrap" -- true
```

Preparation is consent-gated: without `--allow-project-setup` the run refuses as
`denied` before any nix process starts - no readiness query, no evaluation, no
store contact - naming the step, the kind and the installable.

```console
$ devshell-poc prepare --step poc/devshell.toml --out /tmp/x
denied: step 'rust' would evaluate './nix' and run its shell hook on this host, and the project has not consented
  step: rust (devshell)
  remedy: add the project to allowed-projects, or set allow-project-setup
$ echo $?
6
```

## Q1: does a kind the contract does not name survive a real launcher?

**Yes.** The kind is a string on the way in, the settings are a closed enum the
harness keeps, and `Step::kind` is derived from those settings, so the two
cannot disagree. Nothing downstream of preparation sees a kind:

```console
$ rg -n 'kind\(\)' src/
src/model.rs:21:    pub fn kind(&self) -> &'static str {
src/model.rs:22:        self.settings.kind()
src/prepare.rs:102/104/121/…   provenance and failure context: step.kind()
src/main.rs:209:            .find(|step| step.kind() == "devshell")
$ rg -n 'match kind' src/
src/model.rs:53:    let settings = match kind.as_str() {
```

`launch` is given a `View` and an environment; it has no access to a step and
would be identical for any kind that produced the same two contributions. The
declared step is parsed from `poc/devshell.toml` through `parse_sequence`, so
the run above exercises the parse site rather than asserting it.

Cost of the alternative, measured by reading the crate:

| Site | What it does today |
| --- | --- |
| `src/sandbox/contract.rs:74` | `ProvisionKind` is a closed enum |
| `src/sandbox/contract.rs:117` | `ProvisionStep` is a closed enum, settings inline |
| `src/provision/resolve.rs:263` | `step()` matches every variant to clone it |
| `src/backends/docker/backend.rs:322` | `run_steps` destructures every variant |
| `src/config.rs` tests | an unknown kind is asserted to fail at parse |

One parse arm beside one settings variant costs one site here; the same shape in
the crate would cost three match sites plus configuration tying. T-027 and T-032
settle where that lives: a registry outside the contract, with the kind kept as
a string.

## Q2: what does a backend have to report?

**Two different things, to two different questions.** The harness now answers
both from code rather than reading:

- The host asks whether the step can be prepared here. `checks::readiness`
  reports `nix`, `flakes` and `store`, each with a class, a reason and a remedy,
  and evaluates nothing. Enabling flakes off, or removing nix from PATH, changes
  the reported check and the exit class (7, `unavailable`).
- The sandbox asks whether the contributions can be applied. `requirements`
  checks a sandbox requirement against the same `View` the launcher mounts, and
  a host requirement against this machine. Each scope has its own oracle, so a
  path that exists on the host is still refused for the sandbox:

```console
$ devshell-poc run --require-path /usr/bin …   # unsupported, backend named
$ devshell-poc run --require-host-path /usr/bin …  # satisfied
```

A backend therefore needs no kind support at all once preparation is host-side;
it needs contribution support, which for this ADR is "environment and mounts".
T-028 and T-031 settle the payload. What remains open is whether contribution
support is a static claim per backend or derived from readiness, not whether
kinds belong in the backend.

No production backend was exercised: the harness applies the contributions with
`bwrap` directly, because `lbx-bwrap` refuses reads under `/nix`. So AC-015's
backend half is unproven here, and the follow-up must prove that a contract
backend applies an environment and a set of mounts without importing the kind.

## Measurements

| Measurement | Value |
| --- | --- |
| print-dev-env exported variables | 104 |
| variables the entered devShell sets | 106 |
| contributed after filtering | 97 |
| added by the shell hook | 4 (`CARGO_HOME`, `NIX_GCROOT`, `RUSTFLAGS`, `RUSTUP_HOME`) |
| overridden relative to the baseline | 3 (`NIX_LDFLAGS`, `PATH`, `out`) |
| dropped as shell state or restored | 9 (`HOME`, `NIX_BUILD_TOP`, `PWD`, `SHLVL`, `TEMP`, `TEMPDIR`, `TMP`, `TMPDIR`, `_`) |
| baseline names the entered shell lost | 5 (`NIX_ENFORCE_PURITY`, `NIX_LOG_FD`, `OLDPWD`, `TERM`, `TZ`) |
| rejected capture records | 0 |
| caller credentials present | none (`SSH_AUTH_SOCK`, `DOCKER_HOST`, `DBUS_SESSION_BUS_ADDRESS`, `WAYLAND_DISPLAY` all absent) |
| `print-dev-env` | 4.3–4.9 s |
| entering the devShell | 3.9–5.9 s |
| closure | 238 store paths |
| mounts, closure view | 244 (238 store paths + 6 host files) |
| mounts, whole view | 7 (the store root + 6 host files) |
| bwrap argv, closure view | 32,861 bytes |
| bwrap argv, whole view | 657 bytes |
| bwrap launch | 19–1042 ms |
| store root on the output path | held (`nix-store --add-root --indirect`) |
| workload | `cargo test -p agentkit-common` passes, 6 tests |

The baseline names 104 variables and the entered shell sets 106; nine are
dropped as shell state or restored, leaving 97 contributed. The
exported-but-empty build-phase variables are among the 97, which is what T-029
recorded. `NIX_BUILD_TOP` is dropped rather than contributed: its entered value
names a host scratch directory the sandbox does not have, which is what T-030's
six-name restore rule asks for.

The prepared environment is 97 variables against T-011's 91 and the earlier
prototype's 80. The difference is the rule: the earlier prototype dropped 18
names `print-dev-env` marks exported with an empty value, and this one keeps
them, because `nix develop` sets them and direnv restores them.

The run used 244 `--ro-bind` pairs, `--unshare-net`, `--disable-userns`,
`--new-session`, a read-only store, the repository bound read-write at its host
path, and six host files bound individually (`/etc/passwd`, `/etc/group`,
`/etc/nsswitch.conf`, `/etc/resolv.conf`, `/etc/hosts`, `/etc/ssl/certs`). `/etc`
is not bound wholesale and the system profile is not bound at all, so no system
nix binary is reachable; the closure contains none.

The contributed environment reaches bwrap through its own environment, never
through `--setenv`. argv is world-readable, so a hook-exported value on the
command line is visible to every local user through `ps`; the environment of a
process is readable only by its owner. That also halves the argv: 32,861 bytes
for the closure view and 657 for the whole-store view.

## Refuted or corrected

1. **`lbx-bwrap` cannot take the closure.** Its `validate` rejects read grants
   that overlap `/nix` (the `PROTECTED` list) and binds the store itself. Feeding
   it the closure is refused before launch, so the prototype drives `bwrap`
   directly with T-016's invocation. The plan's "the existing launcher already
   takes exactly this" was wrong.
1. **One sanitised `nix develop --command env` is not the 91-variable
   differential.** It sets 106, including the 18 variables `print-dev-env` marks
   `exported` but empty — stdenv build-phase state (`cmakeFlags`, `patches`,
   `strictDeps`, `deps*`). `nix develop` sets them and nix-direnv restores them,
   so they are the devShell's environment and the harness contributes them.
   T-011's 91 dropped five provider-supplied names and four shell-state ones; the
   backend supplies those, and the harness drops them here.
1. **`agentkit-path` is not a workspace package.** The tasks named it from the
   earlier spike's fixture; the run uses `agentkit-common`.
1. **`bwrap` is not on the devShell's PATH.** The harness resolves it to an
   absolute path (`nix build --no-link --print-out-paths nixpkgs#bubblewrap`), so
   a PATH entry cannot substitute it either.
1. **`TERM`, `TZ`, `NIX_ENFORCE_PURITY` and `NIX_LOG_FD`** are in the baseline
   but absent from the entered shell; they are not contributed.
1. **A dirty tree writes to the store.** `print-dev-env` copies the flake source
   and builds a source derivation, so the output path differs between a clean and
   a dirty checkout (`diff -r` on the tree changes `/nix/store/…-nix-shell-env`).
   A create that fails after this leaves that store write behind; the refusal
   says so rather than claiming nothing was written.
1. **`path-info` refuses an unrealised devShell without building it.** Querying
   the output path of a devShell that has never been entered exits non-zero and
   writes no output path, so validity is a real gate and the harness refuses
   there before the hook runs (`unavailable`, remedy `nix develop <installable>`).
1. **No shell state crosses.** The capture is `env -0` from the entered shell:
   every record is a well-formed `NAME=value`, with no `BASH_FUNC_*` record and
   no alias or option. A hook that prints a banner cannot corrupt it — records that
   are not assignments are rejected and counted, and the unit tests pin that.
1. **A sanitised HOME costs evaluation time.** `print-dev-env` under a throwaway
   HOME has no nix evaluation cache: 4.3–4.9 s here against 480–520 ms measured
   with the caller's real HOME. Isolation and evaluation speed trade directly,
   and the follow-up has to decide between an isolated HOME, a shared cache
   directory, or accepting the slower path.
1. **`/etc` wholesale is not needed.** Six individual files suffice for this
   workload; binding `/etc` and the system profile put the host's nix
   configuration, and potentially a nix binary, inside the sandbox.
1. **Validity is checked after evaluation, not before it.** `print-dev-env` is what
   reports the output path the step validates, so it must run first, and on an
   unrealised devShell it can realise the environment derivation and whatever
   that derivation needs. The step refuses before the hook runs, but it cannot
   promise that nothing was realised: the no-realisation guarantee holds where
   the store already has the devShell. The doctor does not evaluate at all, so
   its guarantee is unaffected.
1. **An unrooted closure refuses preparation.** Holding the GC root is not a
   measurement: a closure that can be collected between the query and the mount
   fails create, so a root that cannot be taken is `unavailable` and the run
   stops rather than continuing with a view a collection can empty.

## What the follow-up must decide

Settled by the map and not reopened here:

- Where a kind lives: a registry outside the contract, kind kept as a string
  (T-027, T-032).
- What a backend reports: contribution support, not kind support; the payload
  carries environment and mounts (T-028, T-031).
- Which variables cross: everything the shell sets, minus shell state and
  backend-supplied names, exported-but-empty included (T-029, T-030).

Still open:

- Whether preparation is persisted per sandbox or recomputed per command; the
  prototype prepares once per run, and the GC root it takes has no owner.
- What the closure-size ceiling is. 238 paths cost 51 KB of argv here; a larger
  closure is untested and each backend must refuse against its own limit.
- How a backend rewrites workspace-rooted values (`CARGO_HOME`, `RUSTUP_HOME`,
  the leading `PATH` entry) when it presents the workspace elsewhere. The
  prototype records them and rewrites nothing.
- Whether a hook that execs another shell is detected or recorded as unhandled
  (EC-007). It is unhandled.
- Whether an isolated HOME is worth its evaluation cost, or a shared evaluation
  cache is the better trade.

## Limits

One host, one flake, one workload. No Darwin, no failing-hook run, no command
that needs shell state. Those paths are implemented or specified but were not
exercised here.
