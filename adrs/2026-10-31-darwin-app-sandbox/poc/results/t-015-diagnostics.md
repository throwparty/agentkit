# T-015 denial diagnostics

A denial is reported in four places. The third is the one that makes attribution
per-command possible.

## 1. Errno in the child

```
touch: /Users/lukecarrier/lbx-tag-nope: Operation not permitted
exit=1
```

`EPERM`, not `EACCES`. The command sees a normal syscall failure and can react to
it, which is why the fixture can report `denied` per check.

## 2. Kernel log line

```
2026-10-31 03:19:42.783 E  kernel[0:173bdd] (Sandbox) Sandbox: touch(14317)
deny(1) file-write-create /Users/lukecarrier/lbx-tag-nope
```

Process name, pid, operation and path. Requires `--info --debug`; without them
nothing is returned at all. The predicated query was:

```
/usr/bin/log show --last 2m --info --debug --style compact \
    --predicate 'eventMessage CONTAINS "lbx-tag"'
```

## 3. Profile tag

A rule written as `(deny default (with message "lbx-tag"))` makes the tag
appear as its own line next to the denial. That is the mechanism for attributing
a denial to a specific sandbox or command: embed an identifier in the message and
filter the log by it. Claude Code's sandbox runtime does exactly this, encoding
the command in the tag.

## 4. Structured metadata

`sandboxd` emits a JSON `MetaData` object per denial with `operation`, `pid`,
`process-path`, `errno`, `profile-flags` and the responsible process. Richer than
the kernel line, and it names the responsible application, which is how a
supervisor could distinguish its own sandbox from unrelated noise.

## Corrections to earlier notes

- Denial attribution **does** work. The earlier conclusion that it did not was
  wrong on two counts: `log` in zsh is a shell builtin, so `log show ...` fails
  with `too many arguments` unless you call `/usr/bin/log`; and without
  `--info --debug`, the kernel sandbox messages are filtered out.
- `(with message ...)` is not only a debugging aid; it is the only per-sandbox
  identifier available, so a production wrapper should emit one per sandbox
  rather than a fixed tag.

## Tag support

The custom helper now emits `(deny default (with message "<tag>"))` from
`--tag`, defaulting to `litterbox`. nono cannot set the deny-default message
because it owns the profile; the flag is accepted for CLI parity, warns, and is
recorded as gap 7 in T-011.
