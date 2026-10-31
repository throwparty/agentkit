# Startup cost (NFR-004)

`poc/timing.py <binary> <label> 30`. Each sample is a full helper invocation that
applies the profile and execs `/usr/bin/true`, measured from outside with
`time.perf_counter`, so it includes process spawn, path canonicalisation, profile
render, `sandbox_init` and the child's exit.

| Prototype | n | bare exec | min | median | p95 | max |
|---|---|---|---|---|---|---|
| Custom | 30 | 2.0ms | 11.9ms | 12.2ms | 13.1ms | 14.4ms |
| Nono | 30 | 2.1ms | 14.2ms | 14.5ms | 15.6ms | 18.9ms |

## Reading

- The per-command cost is around 15ms and is dominated by process spawn and
  profile compilation, not by the library. The 39s cold build for nono is a
  build cost, not a runtime cost, and does not predict command latency.
- The bare-exec column is the same loop calling `/usr/bin/true` directly, so the
  difference is the cost of confinement: about 10ms for the custom helper and
  12ms for nono.
- p95 is nearest-rank with `ceil`, not `int`, so it is the 95th percentile of 30
  samples rather than the 93rd.
- Both figures are well inside what a process-per-command design can absorb. The
  cost that matters for that design is the command itself, not the confinement.
