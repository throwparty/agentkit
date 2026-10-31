# Acceptance evidence

Each criterion, the command that exercises it, and the verdict.

| Id | Criterion | Evidence | Verdict |
|---|---|---|---|
| AC-001 | Write outside the workspace denied | `poc/run.sh`, hostile `write-outside` | proven |
| AC-002 | Credential read denied, including with the workspace inside `$HOME` | `poc/run.sh` hostile checks; `poc/inside-home.sh` grants the whole home and places the workspace inside it — key, keychain and `..` reads all denied | proven |
| AC-003 | Network denied by default | `poc/egress.sh`, `no-grant` → `eperm` | proven |
| AC-004 | A grant is specific | `poc/egress.sh`, `second-port` → `eperm` while the granted port connects. Host specificity is **not expressible**: the profile language accepts only `*` or `localhost` as a host token, so an IP-addressed rule does not compile | partial — port proven, host not expressible |
| AC-005 | Descendants confined | `poc/run.sh`, hostile `child-write` | proven |
| AC-006 | Pause and resume | `poc/lifecycle.sh` freezes and resumes a live loop | harness-demonstrated; prototype API out of scope by decision |
| AC-007 | Delete kills and cleans | `poc/lifecycle.sh`: leader gone, 0 sentinel survivors, workspace removed and asserted absent | harness-demonstrated; the removal is performed by the harness, not the prototype |
| AC-008 | Fail closed | `poc/failclosed.sh`: seven invalid inputs, each exits 64 with the command not run | proven |
| AC-009 | Docker path unaffected | No file outside the ADR directory changed; the crate's tests pass | not verified — no container was created; the claim rests on the diff and the unit tests |
| AC-010 | Questions answered | `findings.md` | proven |
| AC-011 | Applies unsigned | `poc/results/t-005-apply.md` | proven |
| AC-012 | Profile off the command line | `poc/ps-capture.sh`: profile text absent from every process, command visible | proven |
| AC-013 | Golden profiles | three fixtures: empty network, one connect grant, and a home plus read/write grants that pins the invariant block | proven |
| AC-014 | Nono exercised | `poc/results/t-012-nono.md`, hostile checks recorded | proven |

## Notes

- AC-004's host clause is not a gap in the implementation; it is a limit of the
  mechanism. `sandbox-exec` rejects `(remote ip "127.0.0.1:8765")` and
  `(remote tcp "example.com:443")` with "host must be * or localhost". Only
  `*:port` and `localhost:port` compile. Egress beyond loopback is therefore all
  or nothing, which is the strongest argument in the findings for a proxy.
- AC-006 is a decision, not a failure: supervision was ruled out of scope for
  the spike.
