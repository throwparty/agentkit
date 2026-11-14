# Implementation notes

Details that the specification does not repeat. Evidence for each is in
`maps/2026-10-31-nix-devshell-sandbox/`.

## Working directory

`bwrap --wd <path>` resolves the path in the caller's namespace before entering
the sandbox. Use `--wdns` so the workspace is resolved inside the view; a path
that exists only there otherwise fails silently or resolves to the wrong tree.

## Namespace join

`nsenter` needs `--preserve-credentials` because bwrap denies `setgroups` in its
user namespace. The backend does not use `nsenter`; it joins in-process. Every
joined pid is written into the sandbox scope before `exec`, because a joined
command is a child of the client and `SIGSTOP` on the holder pauses nothing
(T-019).

## Tests

Module tests live beside the code, so select them by package and module:
`cargo test -p agentkit-litterbox --lib backends::bubblewrap`. The shared
integration target is `--test doctor`. Ubuntu CI keeps `cargo test --locked`.

The host suite needs bwrap, unprivileged user namespaces and a cgroup scope. It
skips with the missing requirement named where they are absent; a CI runner that
is supposed to have them treats unavailable as a failure, not a skip.
