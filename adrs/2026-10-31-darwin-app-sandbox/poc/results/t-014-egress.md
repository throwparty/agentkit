# T-014 egress grants

Two local `python3 -m http.server` instances, on 8765 and 8766. No external
dependency.

| Check | Custom | Nono |
|---|---|---|
| granted `localhost:8765` | ok (connected) | ok |
| second local port 8766, same grant | denied (EPERM) | denied (EPERM) |
| no grant at all, to 8765 | denied (EPERM) | denied (EPERM) |
| external host | dns-denied | dns-denied |

The verdict comes from a probe that reports the errno of the connect itself.
curl collapses `EPERM` and `ECONNREFUSED` into `Couldn't connect to server`, so
the earlier harness could not tell a sandbox denial from a refused connection.

The second server makes the negative case real: it is accepting connections, so
the only reason the check fails is the sandbox.

## What the failure looks like

- Ungranted port: `connect` returns `EPERM`, not `ECONNREFUSED`. The socket call
  is denied before the kernel tries.
- External host: name resolution fails first, recorded as `dns-denied` rather
  than scored as a connect denial.

## Consequence

A grant fixes a port on loopback. The host token is not a caller choice: only
`localhost` and `*` compile, and `*` means every host. `localhost:8765` works; `example.com:443` cannot be granted, because
resolving the name is itself a network operation the profile denies.

That is the mechanical reason per-destination egress over the public internet is
a proxy problem rather than a policy problem: the proxy is the only component
that can resolve names and make decisions while the sandboxed process talks to a
single address it is allowed to reach.

nono reaches the same result only by accepting a raw `platform_rule` (T-011,
gap 2).
