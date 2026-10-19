# agentkit-credentials

The shared credential-helper crate: one crate, two roles.

## What lives here

```
src/lib.rs        crate root: read_stdin (helper-server side) + module docs
src/consumer.rs   the consumer side of the helper protocol (library)
src/bin/          the helper binaries (server side)
  agentkit-credential-keychain.rs
  agentkit-credential-file.rs
tests/            integration tests for each helper binary
```

**Server side.** The helper binaries store and return secrets. They are
invoked by a component as
`agentkit-credential-<name> <command> <component> <identity>`, reading an
optional JSON body on stdin and answering on stdout with JSON carrying at
least `access_token`. Each binary is independent; only the keychain binary
depends on `keyring-core`.

**Consumer side.** [`consumer`] is the client for that protocol, shared by
every component that resolves credentials:

- `helper_binary_name`, `resolve_helper_path` — locate the helper (sibling
  of the running executable, then `PATH`)
- `run` — invoke it with a command, component, identity, and optional stdin
- `Credential::parse` — parse the JSON answer
- `parse_marker` — parse a whole-value `{cred:IDENTITY}` marker

## Dependencies

- `serde_json` — helper JSON and the consumer's `Credential` parse
- `agentkit-path` — the data directory the file helper writes to
- `keyring-core` (+ per-platform store) — the keychain helper only
- `fs2` — file-helper locking

The consumer module depends on none of the helper-only crates; it uses
`std` and `serde_json` only.

## Consumers

- `agentkit-tackle` — `providers/` (endpoint auth) and `mcp/` (`{cred:…}`
  env markers at spawn)
- `agentkit-switchboard` — `credential/` (provider auth, OAuth refresh)

Both use the consumer library; neither links the helper binaries' code.
