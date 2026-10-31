# T-012 nono end-to-end run

Same fixture and harness as T-009, nono 0.81.0 via
`poc/run.sh ./poc/nono/target/release/sandbox-nono nono nono`.

## Workload

```
cargo build, cargo test, cargo run
fixture: 5
```

## Hostile checks

```
write-outside: denied
write-home: denied
read-ssh-key: denied (stat refused)
read-keychain: denied
connect: denied
child-write: denied
```

`read-ssh-key` reports `denied (stat refused)`: nono's profile denies the stat
itself, so the fixture distinguishes that from an absent fixture rather than
treating `exists() == false` as a pass.

## Harness warnings, not failures

```
sandbox-nono: baseline skipped (absent): /private/var/db/dyld
sandbox-nono: --tag nono not applied: nono owns the profile (gap 7)
```

Both are recorded gaps rather than swallowed output.

## Differences from the custom run

| | Custom | Nono |
|---|---|---|
| build + test + run | ok | ok |
| six hostile checks | all denied | all denied |
| shell startup | silent | `Error opening /private/var/select/sh` (gap 4) |
| baseline | fixed list | filtered against the host, one warning per skip |
| `/dev/null` writes | profile model | raw `platform_rule` (gap 1) |
| connect grants | native | raw `platform_rule` (gap 2) |
| denial tag | `--tag` applied | not expressible (gap 7) |
| dependencies | none | 203 crates |
| cold build | 0.9s | 39s |
