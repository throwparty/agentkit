# Implementation notes

Details that the specification does not repeat. Evidence for each is in
`maps/2026-10-31-nix-devshell-sandbox/`.

## Promoted helper

`poc/custom` under `adrs/2026-10-31-darwin-app-sandbox` becomes
`litterbox-seatbelt-helper`. Keep its `policy.rs` and its tests; replace the
hardcoded store baseline, the environment allowlist and the one-profile-per-run
entry point. Its `findings.md` reproduce commands remain valid for the promoted
binary.

## Where the profile is applied

Only the holder's forked child calls `sandbox_init`. The server never applies a
profile and never uses `pre_exec`; T-049 measured that fork+apply works in a
threaded process but POSIX does not permit relying on it.

## Tests

The backend module is `backends::seatbelt`, so its module tests are
`cargo test -p agentkit-litterbox --lib backends::seatbelt`. The shared
integration target is `--test doctor`; add `--test seatbelt` alongside it for
the host suite.

No macOS test job exists yet. When one lands, it runs only those selections, on
`paths:` triggers, and treats unavailable as a failure on a dedicated runner.
The devShell evaluates on `aarch64-darwin`, so the job reuses `nix develop ./nix`
rather than a second toolchain.
