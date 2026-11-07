---
description: 'Working knowledge of Nix for Litterbox work: store paths, closures, devShells, and the difference between evaluation, realisation and querying. Load when running nix on the host, reading a devShell, or reasoning about what a flake build does.'
user_invokable: true
model_invokable: true
---

# Nix

## When to use me

Use me when work touches nix itself: running `nix` on the host, reading a
devShell's environment, deciding whether something is realised, or explaining
what a store path or closure is.

The words are in `{{ $.dir }}/glossary.md`. Read it before naming anything after
Nix — a config key, an identifier, an error message. This prompt is the
behaviour; that file is the vocabulary. Neither restates the other.

## Mental model

1. **The store is the only durable thing.** Every build output is an immutable
   `/nix/store/<hash>-<name>` path. References between paths are absolute, so a
   sandbox that mounts the store at the same path keeps them valid.
1. **Evaluation, realisation and querying are three different acts.**
   Evaluation reads Nix code; realisation makes store paths valid, by building
   them or substituting them from a binary cache; querying reads the store
   database. `print-dev-env` and `develop` can realise; `path-info` does not.
1. **A closure is a graph, not a list of tools.** It is whatever the path
   references, transitively. It is derived, never minimal, and it changes when
   the flake changes.
1. **A devShell is an environment on top of the caller's.** The
   `print-dev-env --json` capture is the baseline; `nix develop` then runs the
   shell hook, which adds and overrides, inheriting whatever the invoking shell
   exported. The differential environment is the union: **every exported
   baseline variable**, plus the post-hook value of every variable the hook
   changed, captured under a sanitised caller. Never drop a baseline export
   because the hook left it unchanged.
1. **Flake evaluation is pure; the hook is not.** Evaluation sees only declared
   inputs. The hook runs as bash with the caller's environment, so its output
   depends on who ran it. `print-dev-env --json` reports the baseline without
   running the hook.

## Verified facts

Host: Nix 2.34.9, Linux. Re-run the commands in *Re-verifying* before trusting a
number.

- **`print-dev-env --json` shape.** For `./nix` in this repository it reports
  153 variables, of which 104 are `exported` — 85 carrying a value and 19
  legitimately empty — and 109 `bashFunctions`. Each variable is
  `{type, value}` with `type` one of `exported`, `var`, `array`. The 49
  non-`exported` entries carry stdenv build-phase state that is wrong to
  inherit. Retain empty exports: a process distinguishes an empty value from an
  absent one.
- **`nix path-info` does not realise.** Against a derivation that has never been
  built it exits non-zero with `path '/nix/store/…' is not valid`, and the only
  store entry it creates is the `.drv`. It is the validity check to use when a
  build must not happen.
- **The closure of this repository's devShell is 238 paths**, matching the map
  evidence (T-022). The host store holds tens of thousands, so the closure is
  the narrow view.
- **No nix CLI is in that closure.** The only path whose name contains `nix` is
  the devShell environment itself, `…-nix-shell-env`. A grep for the CLI must
  anchor on `-nix-[0-9]`, or it matches the env path.
- **`print-dev-env` can build a source copy.** On a dirty Git tree it built a
  `source.drv` and copied inputs from the cache before printing, even though the
  devShell environment was already valid. "No realisation" means no realisation
  of the devShell; the flake source copy is still a store write.
- **`BASH_ENV` reaches bash, not `sh`.** `BASH_ENV=file bash -c 'type fn'`
  resolves a function defined in the file. The same with `sh -c` fails, and on
  this host `sh` *is* bash — bash invoked as `sh` does not read `BASH_ENV`.
- **`nix develop --command env` carries the caller's whole session.** Recorded in
  `maps/2026-10-31-nix-devshell-sandbox/evidence/T-011.md`: SSH and Bitwarden
  agent sockets, `DOCKER_HOST`, the session D-Bus address and the Wayland
  socket all appear in it. Never copy it.

## Traps

- `print-dev-env` is an experimental command: its interface may change between
  Nix releases. Pin the version you verify against.
- Dependency order in `nix path-info -r` output is not sorted and not promised.
  Never depend on it.
- A devShell's shell hook runs under bash, and its functions are bash functions.
  Another shell sees neither the functions nor, for `sh`, `BASH_ENV`.
- A dirty working tree changes the flake source hash, so store paths differ from
  a clean checkout's. Two runs on the same tree are consistent; a commit is a
  different flake.

## Re-verifying

```bash
nix --version
nix print-dev-env ./nix --json > /tmp/devenv.json
jq -r '(.variables|length), ([.variables|to_entries[]|select(.value.type=="exported")]|length), ([.variables|to_entries[]|select(.value.type=="exported" and (.value.value|length)==0)]|length), (.bashFunctions|length)' /tmp/devenv.json
ENV=$(jq -r '.variables.out.value' /tmp/devenv.json)
nix path-info -r "$ENV" | wc -l
nix path-info -r "$ENV" | grep -E '/nix/store/[a-z0-9]+-nix-[0-9]' || echo "no nix CLI in closure"
printf 'probe_fn() { echo hi; }\n' > /tmp/probe.bash
BASH_ENV=/tmp/probe.bash bash -c 'type probe_fn'
BASH_ENV=/tmp/probe.bash sh -c 'type probe_fn'   # expected to fail
```

For the validity check without a build, point `nix path-info` at an unbuilt
derivation and confirm it errors with `is not valid` and creates only the `.drv`.

## Where the truth lives

- `{{ $.dir }}/glossary.md` — the words, and the boundary between upstream and
  ours.
- [Nix reference manual](https://nix.dev/manual/nix/2.34/) — complete behaviour;
  this prompt is a subset.
- `maps/2026-10-31-nix-devshell-sandbox/evidence/` — the experiments this
  repository already ran.

## What this does not cover

nixpkgs conventions, NixOS modules, daemon configuration, and evaluation
performance. Add a fact here only when its command is runnable on a supported
host.
