# Code Review Report

**Run ID:** 2026-11-08-pr-365-nix-devshell-poc

**Scope:** d6192bb4eafc0f6a00d6be62a1249606a21ce0dc..ea7c8ce31bc678294d62ace367b9548b008350e6

**Files:**
- `.gitignore`
- `adrs/2026-11-07-devshell-environment/contributions.mmd`
- `adrs/2026-11-07-devshell-environment/create-sequence.mmd`
- `adrs/2026-11-07-devshell-environment/findings.md`
- `adrs/2026-11-07-devshell-environment/plan-architecture.mmd`
- `adrs/2026-11-07-devshell-environment/plan.md`
- `adrs/2026-11-07-devshell-environment/plan.toon`
- `adrs/2026-11-07-devshell-environment/spec.md`
- `adrs/2026-11-07-devshell-environment/spec.toon`
- `adrs/2026-11-07-devshell-environment/tasks.md`
- `adrs/2026-11-07-devshell-environment/tasks.toon`
- `adrs/2026-11-07-devshell-environment/poc/prepare/Cargo.lock`
- `adrs/2026-11-07-devshell-environment/poc/prepare/Cargo.toml`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/closure.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/requirements.rs`
- `adrs/2026-11-07-devshell-environment/poc/prepare/src/util.rs`
- `crates/agentkit-litterbox/CONTEXT.md`
- `maps/2026-10-31-nix-devshell-sandbox/HANDOVER.md`
- `maps/2026-10-31-nix-devshell-sandbox/map.md`
- `maps/2026-10-31-nix-devshell-sandbox/map.toon`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-027.md`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-028.md`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-029.md`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-030.md`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-031.md`
- `maps/2026-10-31-nix-devshell-sandbox/evidence/T-032.md`

**Reference:** spec://adrs/2026-11-07-devshell-environment/spec.md, spec://adrs/2026-11-07-devshell-environment/tasks.md

---

## Summary

| Agent | Role | Findings |
|-------|------|----------|
| Paige | Product Reviewer | 21 |
| Archie | Architecture Reviewer | 20 |
| Quest | QA Engineer | 17 |
| Scout | Security Engineer | 18 |
| Ollie | Operations Engineer | 18 |

**Total:** 68 findings

---

## Findings

### doctor-evaluates-and-prepares

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:37` |
| Observed | host_checks runs nix print-dev-env to test that the installable evaluates, which evaluates the flake, may fetch unlocked inputs and writes the flake source to the store. FR-009, FR-015 and AC-011 describe readiness as a store query that neither prepares the devShell nor creates a store path. |
| Expected | Readiness is a non-mutating query, or the requirement states the evaluation and its store write and doctor becomes consent-gated. |
| Impact | A doctor run on an untrusted checkout performs evaluation and network egress, and a refused create has already written store paths; the no-mutation guarantee the spec asserts is false and cannot be tested as written. |
| Recommendation | Separate evaluation from readiness, or restate FR-015 as no sandbox, workspace, branch or devShell realisation and enumerate the permitted store writes. |

---

### host-readiness-flakes-unchecked

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:14` |
| Observed | host_checks implements nix --version, print-dev-env and path-info only. No flakes-enabled check exists, though FR-004, FR-009, EC-004, plan.md and findings.md all name one. |
| Expected | A dedicated check ahead of evaluation that reports the disabled experimental feature and the setting that enables it. |
| Impact | With flakes disabled the failure is attributed to the wrong requirement and the printed remedy also fails, so doctor and create cannot be distinguished by a caller or a test. |
| Recommendation | Add the flakes check first, name the setting in its remedy, and cover EC-004 with a test. |

---

### no-deadline-on-host-subprocesses

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/util.rs:24` |
| Observed | Every child runs through Command::output or .status with no deadline and with output captured into memory: print-dev-env, nix develop, path-info -r and bwrap. |
| Expected | The create budget the crate already applies to provisioned commands, with expiry naming the step and the budget. |
| Impact | A blocking shellHook, a network-fetching flake or a wedged nix daemon hangs create indefinitely and silently, and nothing records which step was stuck. |
| Recommendation | Give each host invocation a deadline and bounded capture, and report the primitive, the step and the elapsed budget on expiry. |

---

### capture-path-shell-injection

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:55` |
| Observed | The NUL capture path from --out is interpolated unquoted into sh -c with format! of capture.display. |
| Expected | The path is passed as data, either as a positional argument to a fixed script or as an inherited file descriptor, and never formatted into shell source. |
| Impact | Any --out path with a space, quote or metacharacter either breaks the redirect, producing a message that blames the project hook, or executes in the harness shell as the caller. |
| Recommendation | Remove the sh -c string build; open the file and duplicate it onto fd 1, or pass the path as $1. |

---

### caller-home-exposed-to-hook

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:44` |
| Observed | The sanitised caller sets HOME to the real home, so an untrusted flake evaluation and shellHook run with ~/.ssh, ~/.aws, ~/.config/gh, ~/.netrc and ~/.git-credentials readable. |
| Expected | Evaluation runs with a disposable HOME and no XDG or GIT_CONFIG_GLOBAL reference into the real one. |
| Impact | A hostile devShell hook reads and exfiltrates user credentials and keys, so consent to evaluate a flake silently becomes consent to expose the home directory; this defeats the containment the sandbox exists for. |
| Recommendation | Create a throwaway HOME per prepare, redirect XDG_CONFIG_HOME, XDG_CACHE_HOME and GIT_CONFIG_GLOBAL into it, and record the changed hook behaviour. |

---

### contribution-filter-is-denylist

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:67` |
| Observed | Contributions are every variable the entered shell sets except a nine-name denylist and build-phase names. The credential list at prepare.rs:140 is printed but never enforced, and AC-001 asserts only five names. |
| Expected | An allowlist of devShell-relevant names, or a refusal when a credential-shaped value or an unexpected name survives. |
| Impact | The project hook, or a compromised flake input, exports host secrets into the sandbox environment where a prompt-injected agent reads them and rewrites them into the workspace. |
| Recommendation | Invert the filter to an allowlist derived from print-dev-env plus declared hook changes, and make the credential check a refusal rather than a print. |

---

### environment-drops-empty-exported-vars

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:79` |
| Observed | prepare skips every name print-dev-env marks exported with an empty value. findings.md records 11 dropped, while T-029 records 18 present in this checkout and states they are contributed. |
| Expected | One settled rule, implemented, tested and mirrored in findings.md so the follow-up ADR reads a single record. |
| Impact | The prepared environment diverges from nix develop and direnv, for example under set -u, and the production ADR would consume a decision the prototype refutes with no test to catch either direction. |
| Recommendation | Settle the policy, keep the exported-but-empty variables if T-029 stands, update the counts in findings.md, and pin the surviving name set with a fixture test. |

---

### environment-rule-differs-from-t030

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:45` |
| Observed | prepare runs nix develop --command sh -c env -0 and then applies a baseline-differential denylist. T-030, map.md and HANDOVER.md mandate evaluating the print-dev-env script once and restoring NIX_BUILD_TOP, TMP, TMPDIR, TEMP, TEMPDIR and terminfo, and say the differential replaces the denylist. |
| Expected | The behaviour the evidence records as chosen, with tests for temporary-variable restoration and kept build-phase variables. |
| Impact | TMPDIR, TMP, TEMP and TEMPDIR are dropped rather than restored, NIX_BUILD_TOP survival is untested, and FR-006 contradicts T-030 so the follow-up cannot satisfy both. |
| Recommendation | Implement the nix-direnv rule with tests, or amend FR-006 and mark T-030 as decided but not implemented. |

---

### findings-open-decisions-stale

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/findings.md:108` |
| Observed | The what the follow-up must decide section still lists where a kind lives, what a backend reports, the empty-variable question and persistence as open, though T-027 to T-032 and HANDOVER.md record those as settled. spec.md still says the two questions are unanswered, and findings.md:73 still carries the corrected count. |
| Expected | Only genuinely unresolved items remain, with the answered ones attributed to the tickets that settled them. |
| Impact | Two documents give opposite instructions on the same decision, and a reader who starts from the ADR as the workflow prescribes gets the superseded answer. |
| Recommendation | Replace the open-questions section with the answered questions and their tickets, and reconcile spec.md, findings.md and HANDOVER.md. |

---

### findings-missing-required-refutations

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/findings.md:86` |
| Observed | The Refuted or corrected section records the lbx-bwrap refusal, the variable differential, agentkit-path and bwrap PATH, but not the dirty-tree source write, the path-info refusal or the absence of shell state. |
| Expected | Every refutation T-008 requires, each with the command and an output excerpt. |
| Impact | The follow-up cannot tell whether a dirty tree writes to the store, why path-info refused, or that no shell state crosses, all of which are load-bearing for FR-008 and FR-010. |
| Recommendation | Record each with its command and excerpt, and state the shell-state result in the findings rather than only in Limits. |

---

### prototype-step-never-parsed

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:33` |
| Observed | Step, Settings and parse_step are used only by model.rs tests. main.rs takes --installable directly and never constructs a Step, so the declared-step-to-launcher path is never exercised. |
| Expected | A binary that takes a declared step, so the kind the contract does not name is proven to survive a real launcher by running it. |
| Impact | The claim that a generic step survives a launcher rests on code reading rather than a run, and the harness never demonstrates the boundary it exists to test. |
| Recommendation | Add a step file or CLI input that parses through parse_step and drives prepare and launch from the parsed step, then re-run the question. |

---

### poc-only-parse-tests

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:86` |
| Observed | The crate has five tests, all parse_step cases. checks.rs, closure.rs, requirements.rs, util.rs, prepare.rs, launch.rs and main.rs carry no cfg(test) and no assertions. |
| Expected | T-003 requires a test per host check, T-004 the hook cases, T-005 the mount list, T-006 the launch environment, T-007 one test per scope, and NFR-006 recorded-output fixtures for parsing and contribution construction. |
| Impact | Six of seven modules are untested, so every count, refusal, mount-set and environment claim rests on a single manual run. |
| Recommendation | Add module tests over recorded fixtures for print-dev-env JSON, path-info output and env -0 bytes, and extract an argv-builder seam so launch is testable. |

---

### poc-tests-not-in-ci

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | HIGH |
| Location | `code://.github/workflows/agentkit.build-and-test.yaml:65` |
| Observed | CI runs cargo test --locked at the workspace root and the root Cargo.toml excludes adrs, so the PoC is never built, linted or tested by any job. |
| Expected | Some job or recipe still exercises the crate even though it stays outside the workspace. |
| Impact | The only tests that exist are never executed, and toolchain, edition 2024 and dependency bumps break the crate silently. |
| Recommendation | Add a workflow step or recipe running test and clippy in the PoC directory, plus a check that adrs stays excluded. |

---

### contribution-filter-untested

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | HIGH |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:74` |
| Observed | Contribution construction is inline in a function that spawns nix: the denylist, the empty-export and build-phase drops, and the added, overridden and missing classification. baseline is private and untested. |
| Expected | Parsing and contribution construction unit-tested against recorded print-dev-env output with no nix and no network, as NFR-006 requires. |
| Impact | The single behaviour this ADR specifies, what crosses into the sandbox, has no test, and it is exactly the policy still open between T-029 and T-030. |
| Recommendation | Extract filter and baseline as pure functions and pin them with recorded print-dev-env and entered-environment fixtures. |

---

### prepare-evaluates-flake-twice

| Field | Detail |
|-------|--------|
| Axis | performance |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:24` |
| Observed | prepare calls host_checks, which runs nix print-dev-env --json, then runs the identical command again and discards the first result. Each evaluation is about half a second. |
| Expected | One evaluation whose parsed baseline, output path and validity feed both readiness and preparation. |
| Impact | Host preparation time roughly doubles, nix daemon load doubles, and the two evaluations can observe different store states. |
| Recommendation | Return the parsed print-dev-env result from the check and pass it into prepare, timing the check phase separately. |

---

### evaluation-env-inherited-by-nix-calls

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:31` |
| Observed | Every nix invocation except the devShell entry calls run_command with envs None, inheriting NIX_CONFIG, NIX_PATH, NIX_REMOTE, NIX_SSL_CERT_FILE, TMPDIR and GIT_*, while the entered shell runs under env -i with only HOME and PATH. Binaries resolve through PATH. |
| Expected | All nix calls run under the same env -i sanitiser as the entry, with binaries resolved to absolute paths, so the baseline is judged against the environment FR-006 describes. |
| Impact | Caller nix configuration redirects what is evaluated and where it fetches from, a writable PATH entry substitutes nix or bwrap, and added versus overridden provenance can be wrong with no diagnostic. |
| Recommendation | Use one sanitised spawn helper for every nix call, resolve binaries once to absolute paths, and record the nix configuration used in provenance. |

---

### consent-gate-absent

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:21` |
| Observed | prepare evaluates any installable with no allowed-projects or allow-project-setup check, and the installable reaches nix develop and print-dev-env verbatim, so a remote ref is fetched and its hook executed. findings.md does not record consent as unexercised. |
| Expected | FR-011 and AC-006: a Denied refusal naming the step identity and both settings before any evaluation, query or backend call. |
| Impact | Running the harness, or a production port of it, on a cloned repository executes that repository code with the user privileges without the mandated consent, so AC-006 rests on nothing. |
| Recommendation | Port the consent predicate ahead of prepare in the follow-up and refuse remote installables outside allowed-projects, or record the omission in findings.md. |

---

### installable-argv-injection

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:37` |
| Observed | installable and the out value are inserted as bare argv elements after the subcommand with no -- separator and no leading-dash rejection. |
| Expected | The flake reference is validated against a flake-ref grammar and any value beginning with - is refused before spawning. |
| Impact | A project-controlled installable of --option substituters https://evil is parsed by nix as an option, changing evaluation sources or writing a profile to an attacker-chosen path. |
| Recommendation | Reject references beginning with - and validate that the out variable starts with /nix/store. |

---

### closure-members-unvalidated

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/closure.rs:25` |
| Observed | Lines of nix path-info -r output become ro-bind sources with no assertion that each is absolute and under the store, and the queried path comes from the project-controlled out variable in print-dev-env JSON. |
| Expected | Every mount matches the store path shape, and the out value is validated the same way before any query. |
| Impact | The mount set is chosen by the project rather than by the closure rule AC-009 asserts, and a malformed line reaches bwrap as a relative or option-like bind. |
| Recommendation | Validate and canonicalise every mount against the store prefix, reject non-store paths, and assert the count equals the query. |

---

### closure-no-gc-root

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/closure.rs:12` |
| Observed | The closure query and the later bwrap binds are separate steps with no GC root, and the resulting paths are handed to a sandbox that may run for minutes. |
| Expected | The closure is rooted for the sandbox lifetime. |
| Impact | A concurrent garbage collection makes bwrap fail on a missing path or leaves a running sandbox with part of its closure removed, and the failure is opaque rather than a refusal. |
| Recommendation | Hold a per-sandbox GC root in the record and release it on delete. |

---

### validity-check-toctou

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:58` |
| Observed | Validity is checked with nix path-info and nix develop runs separately with no lock or GC root in between, so a collected path is rebuilt by the entry. |
| Expected | The check and the entry are atomic enough that no build occurs. |
| Impact | A realised derivation means project build code and network access run under a path the ADR says must refuse, so Unavailable degrades silently into a build. |
| Recommendation | Take a temporary GC root before the validity query, hold it through prepare, and re-query immediately before entering. |

---

### launch-binds-outside-closure

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs:38` |
| Observed | Every launch ro-binds /run/current-system and /etc on top of the closure alongside the workspace, while findings.md claims the bind set is the closure plus the workspace and that no nix binary is contributed. |
| Expected | The view is the closure, the workspace and minimal pseudo-filesystems, with any extra bind explicitly accounted for and tested. |
| Impact | The sandbox sees far more of the host than the contract describes, including nix configuration and possibly a nix executable under the system profile, so the verified absence of nix is false for a system-installed nix. |
| Recommendation | Derive extra binds from an explicit tested allowlist, account for each in findings.md, and add a test that enumerates the real bwrap view. |

---

### launch-workspace-unvalidated

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs:59` |
| Observed | The workspace is bound read-write and used as chdir with no canonicalisation, no absolute-path requirement and no rejection of root or home or an ancestor of it. |
| Expected | The workspace is canonicalised, required to be the project root, and refused if it is root or an ancestor of a mounted path. |
| Impact | A workspace of / from configuration, a symlink or a discovery bug binds the host root read-write over the read-only view, so sandboxed code can modify anything the user can. |
| Recommendation | Canonicalise and validate the workspace before building argv and refuse anything that is not a strict descendant of an allowed root. |

---

### launch-no-new-session

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs:20` |
| Observed | The bwrap invocation sets --die-with-parent and the namespace flags but not --new-session, so untrusted code keeps the caller controlling terminal. |
| Expected | Untrusted code runs with a new session that detaches the controlling tty. |
| Impact | A sandboxed command pushes characters into the user terminal, the CVE-2017-5226 class, typing commands into the shell that launched the sandbox. |
| Recommendation | Add --new-session beside --die-with-parent and record it in the launcher contract. |

---

### out-dir-untrusted-input

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:51` |
| Observed | env.nul, out and mounts.nul under --out are read back as trusted input, and the documented path is a predictable name in a world-writable directory created with create_dir_all and default permissions. |
| Expected | A per-run directory created 0700 with exclusive semantics, and each capture file opened with O_NOFOLLOW. |
| Impact | Another local user pre-creates the directory or symlinks a capture file to inject an environment or mount list into a later launch, or to have the harness overwrite a victim file. |
| Recommendation | Use a private temp directory with restrictive permissions, open captures with O_NOFOLLOW and O_EXCL, and refuse a pre-existing out directory. |

---

### env-values-written-world-readable

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:111` |
| Observed | env.nul is created under the default umask and holds every contributed value as name=value, is left behind after the run, and is re-read at launch rather than the in-memory map being used. |
| Expected | Value-bearing files are created 0600 in a 0700 directory, removed after use, and NFR-005 is extended to cover on-disk intermediates. |
| Impact | Secrets a hook exported stay readable to other local users and persist in a scratch directory, which is the exposure NFR-005 forbids, and the launched environment is whatever that file holds at launch. |
| Recommendation | Create the file 0600 with restrictive open options, keep the map in memory for launch, and unlink it on both success and failure. |

---

### persisted-values-no-confidentiality

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/spec.md:151` |
| Observed | FR-012 requires the backend to persist the contributed environment in its record payload and T-031 shows that payload carrying name=value pairs, yet no confidentiality requirement is stated for the payload or its rendering. |
| Expected | A stated rule that values persist only in a restrictive metadata store and never in labels, logs, provenance or inspect output, with a test per surface. |
| Impact | The one place the ADR puts secrets at rest is the one it does not constrain, so a backend that labels containers with its record publishes the devShell environment. |
| Recommendation | Add a persistence confidentiality requirement beside NFR-005 and require backends to keep values out of labels and enumeration output. |

---

### requirement-check-uses-different-view

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/requirements.rs:15` |
| Observed | check_requirements tests satisfaction against the closure mounts and workspace only, while launch also binds the system profile, /etc, /proc, /dev and /tmp. A Host-scoped requirement is never checked, only named in the message. |
| Expected | One mount list including the fixed binds, used by both the check and the launch. |
| Impact | The fail-closed path produces false refusals for paths the sandbox does provide, and half the requirement model is decorative. |
| Recommendation | Build the view list in one place, pass it to both the check and bwrap, and check host requirements or drop the scope. |

---

### requirement-refusal-unstructured

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/requirements.rs:25` |
| Observed | check_requirements returns Err of a formatted sentence naming the requirement, scope, reason and remedy, but no backend, no failure class and no testable fields. Scope only chooses a word in the message. |
| Expected | A structured refusal carrying class, requirement, scope, backend, reason and remedy, with the scope selecting the checker rather than the message. |
| Impact | A caller cannot branch on the failure class or assert its fields, the backend is absent from the message, and AC-014 and NFR-003 cannot be tested. |
| Recommendation | Return a structured refusal, split host and sandbox checkers over one Requirement type, and test both scopes plus the satisfied case. |

---

### step-parse-errors-omit-identity-and-kind

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:77` |
| Observed | reject_unknown reports the key and nothing else, the missing-installable error omits the identity, and no duplicate-devshell-in-sequence check exists. |
| Expected | FR-001 and AC-010: an unknown key or missing installable fails naming the step identity and the kind, and a second devshell step is Misconfigured naming both identities. |
| Impact | A configuration error cannot be traced to the step that caused it, and the duplicate rule is unmodelled so the follow-up has no shape to copy. |
| Recommendation | Include identity and concrete kind in every parse error and model the sequence and duplicate check, or record it as out of prototype scope. |

---

### whole-store-flag-ignored

| Field | Detail |
|-------|--------|
| Axis | configuration |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:107` |
| Observed | run_all parses --whole and continues, closure is always called with whole false, and nothing prints which view was used, although usage advertises the flag. |
| Expected | The parsed flag reaches the closure builder, or the flag is refused with a message naming it as unimplemented, and the view in force is reported. |
| Impact | An operator asking for the whole-store fallback silently gets the narrower closure, which is the failure mode FR-007 and EC-005 exclude, and T-005's whole-store run is never exercised. |
| Recommendation | Thread the flag into the closure call and print the store view beside the mount count, or refuse it explicitly. |

---

### contributions-type-missing

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:7` |
| Observed | plan.md states the harness owns Contributions of environment and mounts, and T-027 and T-028 conclude a backend receives contributions rather than a step, but no such type exists: launch takes a map and a path list separately and requirement checks take a third view. |
| Expected | A named Contributions value that prepare returns and launch and the requirement check consume. |
| Impact | The central claim, that the launcher never learns the kind and only sees contributions, is asserted over loose arguments rather than a type, so the follow-up has no demonstrated seam to copy and AC-002 cannot be verified. |
| Recommendation | Introduce Contributions in model.rs, return it from prepare, and pass one value to launch and to requirement checks. |

---

### poc-settings-enum-duplicates-kind

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:14` |
| Observed | Step carries kind as a string and a closed Settings enum keyed by the same two kinds, so the kind exists twice and can disagree, while parse_step branches on the same closed set. |
| Expected | One representation of the discriminator, with settings owned by the kind that produced them. |
| Impact | The evidence for moving away from a closed kind enum is measured against a harness that still encodes a closed set, so the conclusion is weaker than presented. |
| Recommendation | Derive settings from kind by construction or record in findings.md that the harness keeps a closed settings enum and why. |

---

### poc-arg-parser-swallows-unknown-flags

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:69` |
| Observed | flag scans forward to its name and discards every token it passes, run_all has no catch-all arm, and no subcommand rejects an unknown token. Flag order changes the outcome. |
| Expected | A single pass over the arguments that errors on any unknown token and is order independent, as the sibling bwrap spike does. |
| Impact | A mistyped, reordered or misplaced flag is silently ignored and the run proceeds with defaults, which is the class of silent approximation the fail-closed requirements forbid. |
| Recommendation | Parse into a struct once, refuse unknown tokens, and add tests for both flag orders. |

---

### adr-contributions-diagram-orphaned

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/contributions.mmd:1` |
| Observed | contributions.mmd is referenced by no .toon, while spec.md carries the same flowchart inline, so the .toon and .md pair disagree. |
| Expected | Every .mmd in an ADR is the source some .toon references and the .md is rendered from it. |
| Impact | Regenerating spec.md from spec.toon drops the flowchart, and editing spec.md leaves an orphan file that looks authoritative. |
| Recommendation | Add the mermaid reference to spec.toon or delete contributions.mmd and keep the inline block as hand-authored. |

---

### plan-retains-refuted-launcher-claim

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/plan.md:5` |
| Observed | plan.md and plan.toon still state that launch shells out to the lbx-bwrap binary and that it already takes exactly the view, and the architecture diagram still shows that launcher, while findings.md refutes both and launch.rs drives bwrap directly. |
| Expected | The plan records the refutation beside the claim, or the claim is amended. |
| Impact | The follow-up ADR reads the plan as the design of record and would build against a launcher contract that does not exist and a workload package that is not in the workspace. |
| Recommendation | Amend the approach, technologies and launch sections and the diagram with the refutation, or annotate them as superseded by findings. |

---

### no-closure-tests

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/closure.rs:8` |
| Observed | closure has no test: the whole-store branch, de-duplication and the absolute output are unexercised, and the count is asserted nowhere. |
| Expected | A recorded path-info fixture asserting de-duplication, order, count and the whole-store single mount. |
| Impact | The closure claim is a single manual measurement, and dedup, ordering and whole-store regressions go unnoticed. |
| Recommendation | Unit-test the whole view and a fixture listing containing duplicates, asserting order and count. |

---

### checks-output-path-untested

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:37` |
| Observed | output_path is the sole gate on print-dev-env JSON and is untested for absent variables or out and for non-string values; nothing asserts that a PATH without nix exits non-zero with a remedy. |
| Expected | Fixture tests for output_path and a PATH-scrubbed test asserting exit status and remedy text, as AC-005 and T-003 require. |
| Impact | AC-005 is unverified, malformed JSON surfaces as a bare string, and the host check has no regression guard. |
| Recommendation | Add serde fixtures and the absent-nix test, and assert each check's fields. |

---

### no-banner-hook-test

| Field | Detail |
|-------|--------|
| Axis | test-quality |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/util.rs:33` |
| Observed | read_nul parses NUL records and accepts any NAME=value, including a banner line containing an equals sign. |
| Expected | A recorded fixture with banner bytes asserting every record is a well-formed assignment, with a variable-name grammar check. |
| Impact | The corruption class T-011 found is neither reproduced nor guarded, so a hook echoing LOG=x enters the environment. |
| Recommendation | Validate names against the environment-name grammar and pin the behaviour with a banner fixture. |

---

### model-tests-assert-substrings

| Field | Detail |
|-------|--------|
| Axis | test-quality |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:108` |
| Observed | Tests assert only contains of the installable, the extra key and nix; the last passes on an unknown-kind message alone. Nothing covers missing identity or kind, non-string settings, a command-step unknown key or a second devshell step. |
| Expected | Assertions over the full message including identity and kind, plus both kinds rejection paths and the duplicate-step refusal. |
| Impact | The tests pass while the spec naming requirement is unmet, and the duplicate rule is neither modelled nor refused. |
| Recommendation | Assert exact messages, change the code to name identity and kind, and add the missing cases. |

---

### hook-failure-loses-exit-code-and-output

| Field | Detail |
|-------|--------|
| Axis | observability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:58` |
| Observed | A non-zero exit becomes a fixed message plus the first stderr line. The exit code is discarded, only one line survives, and neither the step identity nor the installable appears, so a silent failure prints nothing after the colon. |
| Expected | Failed carries the exit code, a bounded output tail and the step identity, as CONTEXT.md and FR-014 require and as the crate already does elsewhere. |
| Impact | The most common failure is the least diagnosable, and a silent hook failure is indistinguishable from a harness defect. |
| Recommendation | Carry the exit status, a bounded head and tail of output, the installable and the step identity into the failure. |

---

### main-exit-code-collapsed

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:21` |
| Observed | main maps every error to exit 1; failure class, exit code, step and remedy exist only as prose on stderr. |
| Expected | The CLI exit code derives from the failure class, as CONTEXT.md and FR-014 specify. |
| Impact | A missing nix, a denied consent and a broken hook are indistinguishable to a script, CI job or agent that must branch on the class. |
| Recommendation | Return an error type carrying the class and map it to the exit code, keeping the prose as the reason. |

---

### blocking-host-step-in-async-create

| Field | Detail |
|-------|--------|
| Axis | performance |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/spec.md:86` |
| Observed | The host step is specified as std process work inside create, which is async in the orchestrator, and the crate contains no spawn_blocking. Measured cost is one to two seconds of subprocess wait per create. |
| Expected | Either an explicitly async implementation or spawn_blocking around synchronous preparation, stated in the requirement. |
| Impact | Every concurrent create holds an executor thread for the duration, serialising creates in a multi-request server and starving unrelated futures. |
| Recommendation | State in the requirement that preparation runs on a blocking pool so the boundary is decided rather than inherited. |

---

### spec-gives-host-steps-no-budget

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/spec.md:86` |
| Observed | FR-005 puts preparation inside create but no requirement gives the host step a budget, a failure class for expiry or a remedy, and FR-014 omits took too long. The PoC mirrors the omission. |
| Expected | A stated deadline, or an explicit statement that the create budget covers host steps, with expiry naming the step, the primitive and elapsed time. |
| Impact | The follow-up inherits an unbounded create path, so it either invents a timeout the spec does not test or ships without one. |
| Recommendation | Add to FR-004 and FR-014 that host preparation counts against the create budget and that expiry is a named failure with a remedy. |

---

### prepare-children-not-reaped-on-cancel

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:45` |
| Observed | The devShell entry and print-dev-env are spawned with Command::output and no guard ties their lifetime to the harness. Only bwrap gets --die-with-parent. |
| Expected | A drop guard or process group that terminates host children, as the docker backend uses to roll back a half-created sandbox. |
| Impact | A cancelled create leaves an evaluation, and possibly a hook, running against the store and the checkout with no record that it exists. |
| Recommendation | Spawn host invocations in their own process group, kill them from a guard when preparation does not complete, and name the kill in the reason. |

---

### stale-prepared-state-read-as-current

| Field | Detail |
|-------|--------|
| Axis | reliability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:51` |
| Observed | closure reads the out file left by an earlier prepare, ignores the installable, and prepare writes its files non-atomically, so an interrupted or earlier run leaves a directory that looks current for a different installable. |
| Expected | The directory carries the installable and a generation, and closure refuses when they disagree, or derivation happens in the same process as preparation. |
| Impact | A stale closure is mounted silently: the mounts belong to a different devShell and the count-versus-query check still passes because both come from the same stale file. |
| Recommendation | Write a manifest atomically and validate it on read, and remove the directory when preparation fails. |

---

### readiness-report-not-machine-readable

| Field | Detail |
|-------|--------|
| Axis | observability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:75` |
| Observed | report prints ok or MISSING, a name and a detail string to stdout. Check has no reason field, no exit code and no serialisation. |
| Expected | One structured report shared by doctor and create, carrying primitive, reason and remedy, emitted as tracing events. |
| Impact | The doctor cannot be built from this, the report cannot be asserted or diffed, and the host step would be the only part of create emitting untraced prose. |
| Recommendation | Give Check the fields the failure type carries and emit one serialisable report from both paths. |

---

### real-backend-not-exercised

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs:12` |
| Observed | launch invokes raw bwrap. findings.md records that lbx-bwrap rejects read grants under the store and that the prototype drives bwrap directly, so no backend in the litterbox crate applies the contributions. |
| Expected | AC-015 requires a real backend to apply the environment and mounts without importing the kind. |
| Impact | The backend boundary is not proven, only the mechanism, so surviving a real launcher does not establish that a contract backend can apply the contribution set. |
| Recommendation | Exercise a backend create and exec path with the environment and mounts, or record explicitly that this half of AC-015 is refuted and must be re-proven. |

---

### findings-missing-argv-size

| Field | Detail |
|-------|--------|
| Axis | observability |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/findings.md:63` |
| Observed | The measurements table records variable counts, timings, the closure size and the workload, but no bwrap argv size and no whole-store mount count, and argv goes through Command rather than an args file. |
| Expected | The argv size recorded and the launch shown to succeed with it, with a pre-launch check against the limit. |
| Impact | The follow-up has no measured basis for a closure ceiling, and the first failure on a large closure is a raw exec error rather than a refusal naming the number exceeded. |
| Recommendation | Record argv bytes for the closure case and the one-mount whole-store case, and refuse before launch when the limit is exceeded. |

---

### documentation-not-updated

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | LOW |
| Location | `code://docs/docs/user/litterbox/reference/config.mdx:103` |
| Observed | The configuration reference still documents only the command kind, with no devshell installable, store view or devshell consent, and the nix capability prompt carries none of the new vocabulary although CONTEXT.md was updated. |
| Expected | NFR-008: the reference documents the devshell step, installable, store view and consent, and the capability prompt carries the vocabulary. |
| Impact | Users and the follow-up have no documented devshell configuration; the vocabulary exists only in CONTEXT.md. |
| Recommendation | Document the devshell step and store view, add the terms to the nix prompt, or state that NFR-008 is deferred to the implementation ADR. |

---

### findings-answers-lack-command-and-excerpt

| Field | Detail |
|-------|--------|
| Axis | completeness |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/findings.md:23` |
| Observed | The two question answers come from reading the crate, with no command or output excerpt, and the reproduce block covers only the end-to-end run. |
| Expected | T-008: each question answered with the command that produced it and an output excerpt. |
| Impact | The conclusions cannot be reproduced or audited by a reader, so T-008's success criteria are unmet. |
| Recommendation | Add the search or nix commands and matching excerpts for the branch-site counts and the backend-report sites. |

---

### environment-ac001-unverified

| Field | Detail |
|-------|--------|
| Axis | test-coverage |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/prepare.rs:140` |
| Observed | prepare checks only the socket and credential names. It does not assert that PATH leads with the devShell CARGO_HOME entry, nor that a caller RUSTFLAGS is absent from the result, and a surviving credential never fails prepare. |
| Expected | AC-001 and NFR-005: the leading PATH entry and the absence of caller RUSTFLAGS asserted, and the fixture-secret test over rendered provenance and inspection. |
| Impact | Two load-bearing environment acceptance criteria are unproven, a caller RUSTFLAGS regression goes unnoticed, and a leaked credential passes silently. |
| Recommendation | Add the assertions and a fixture-secret test, and make a non-empty leaked list an error naming the variable and a remedy. |

---

### spec-host-requirement-list-inconsistent

| Field | Detail |
|-------|--------|
| Axis | correctness |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/spec.md:13` |
| Observed | The requirement diagram and contributions.mmd list nix, bash, flakes and a valid devShell as host requirements; FR-004 and CONTEXT.md omit bash, and the checks implement neither bash nor flakes. |
| Expected | One normative host-requirement list shared by the diagram, the requirement, CONTEXT.md and the checks. |
| Impact | A reader cannot tell whether a host without bash should be refused, and the implemented check set matches neither wording. |
| Recommendation | Decide whether bash is a host requirement, then align the diagram, FR-004, CONTEXT.md and the checks. |

---

### requirement-prefix-check-unnormalised

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/requirements.rs:15` |
| Observed | A requirement is satisfied when its nominal path starts with a mount or the workspace, without normalisation or symlink resolution. |
| Expected | Both sides are canonicalised, or requirements containing parent traversal are refused, before the prefix comparison. |
| Impact | A requirement the sandbox cannot meet is reported satisfied, so the command fails inside the sandbox instead of create refusing. |
| Recommendation | Canonicalise both sides before comparing, and reject parent-traversal components. |

---

### doctor-no-mutation-claim

| Field | Detail |
|-------|--------|
| Axis | security |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/spec.md:169` |
| Observed | FR-015 and AC-011 say the doctor creates no store path, while FR-010 concedes print-dev-env copies the flake source and FR-009 requires the installable to be evaluated. |
| Expected | The readiness report distinguishes queries that write the store from those that do not, and the acceptance test names the permitted writes. |
| Impact | A test asserting an unchanged store listing cannot hold alongside the required evaluation, so either the test is weakened or the claim is left untested. |
| Recommendation | Restate FR-015 as no sandbox, workspace, branch or devShell realisation and enumerate the permitted store writes. |

---

### poc-not-declared-standalone-workspace

| Field | Detail |
|-------|--------|
| Axis | convention |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/Cargo.toml:1` |
| Observed | The PoC stays out of the workspace only because the root manifest excludes adrs; there is no workspace table and no standalone comment, though both sibling spikes declare it. |
| Expected | The same explicit standalone declaration the sibling spikes carry. |
| Impact | Moving or copying the crate, or relaxing the exclude, produces the believes-it-is-in-a-workspace error, and the recorded intent lives nowhere in the crate. |
| Recommendation | Add an empty workspace table with the standalone comment the sibling spikes use. |

---

### poc-unused-serde-dependency

| Field | Detail |
|-------|--------|
| Axis | structure |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/Cargo.toml:12` |
| Observed | serde with derive is declared but no derive is used; only serde_json is consumed, and the lock file pulls serde_derive, proc-macro2, quote and syn for nothing. |
| Expected | Only the dependencies the harness consumes. |
| Impact | Five extra crates in the lock file and a declared dependency that misstates the harness surface, in the artefact the ADR cites as evidence. |
| Recommendation | Drop the serde dependency. |

---

### poc-lint-config-absent

| Field | Detail |
|-------|--------|
| Axis | convention |
| Severity | MEDIUM |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/Cargo.toml:11` |
| Observed | No lint tables are declared, while every workspace crate declares warnings deny and clippy all warn, and CI lints only workspace members. |
| Expected | The same lint tables declared locally, since the crate cannot inherit the workspace ones. |
| Impact | The prototype escapes the repository warnings-deny policy, and it is the artefact the follow-up will be held to. |
| Recommendation | Add the rust and clippy lint tables, or state in findings.md why the prototype is exempt. |

---

### poc-closure-installable-ignored

| Field | Detail |
|-------|--------|
| Axis | clarity |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/main.rs:50` |
| Observed | The closure subcommand requires --installable, binds it to underscore, and derives the closure from the out directory alone, while usage advertises the flag for every subcommand. |
| Expected | A flag either changes behaviour or is not accepted, with per-subcommand usage. |
| Impact | The CLI contract contradicts the code in the artefact the reproduce block depends on, and the dead binding marks a seam the author knew was unused. |
| Recommendation | Drop the flag from the closure arm or derive the output path from the installable. |

---

### poc-host-check-positional-index

| Field | Detail |
|-------|--------|
| Axis | clarity |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/checks.rs:33` |
| Observed | After pushing the nix check the code tests index zero to decide whether to stop, making the dependency positional and invisible at the call site. |
| Expected | The nix check outcome held in a named value and tested. |
| Impact | Inserting or reordering a check silently changes which one gates the rest. |
| Recommendation | Bind the nix Check to a local and test the named value. |

---

### poc-run-command-envs-unused

| Field | Detail |
|-------|--------|
| Axis | clarity |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/util.rs:9` |
| Observed | run_command takes an optional environment and clears it when present; every call site passes None, and prepare hand-rolls env -i with Command instead. |
| Expected | One helper owns process spawning including environment construction. |
| Impact | The shared helper advertises a capability nothing uses while the one place that needs it bypasses it, so the sanitisation logic exists twice in spirit. |
| Recommendation | Remove the parameter, or add an environment-aware helper and use it for the devShell entry. |

---

### poc-launch-hardcoded-host-paths

| Field | Detail |
|-------|--------|
| Axis | clarity |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/launch.rs:38` |
| Observed | The system profile and /etc are bound unconditionally, the in-sandbox home and temp directory are hardcoded literals, and bwrap is taken only from a flag. |
| Expected | Named constants or arguments with a comment stating what is required by the workload and what is host scaffolding. |
| Impact | launch presents itself as kind-agnostic and reusable while embedding one distribution layout as unnamed literals, so a reader cannot tell what is essential to the view. |
| Recommendation | Hoist the host-provided paths into named constants beside the workspace and note their provenance in findings.md. |

---

### poc-model-settings-kind-field-unused

| Field | Detail |
|-------|--------|
| Axis | clarity |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:8` |
| Observed | Step.kind is public and set by parse_step, but nothing downstream reads it. |
| Expected | The code that measures the kind boundary says so, or the boundary claim is stated over settings alone. |
| Impact | The harness reports the kind once and never uses it, so nothing downstream sees the kind is true by omission rather than by design. |
| Recommendation | Have provenance write the kind, or delete the field and state the claim over Settings. |

---

### branch-count-claim-unenforced

| Field | Detail |
|-------|--------|
| Axis | test-quality |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/poc/prepare/src/model.rs:46` |
| Observed | T-002 requires the number of kind branches recorded; findings.md says one match with one arm per kind without a count, and nothing enforces that parse_step is the only branch. |
| Expected | A stated count and an enforcing check or documented verification command. |
| Impact | The conclusion that the kind costs one parse site rests on review alone and can drift without notice. |
| Recommendation | State the count in findings.md and add a grep-based or arch-lint check on kind usage in the crate. |

---

### findings-reproduce-host-specific

| Field | Detail |
|-------|--------|
| Axis | test-quality |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/findings.md:19` |
| Observed | The reproduce block hardcodes a store path to bubblewrap and an unqualified temp out directory. |
| Expected | A host-independent way to locate bwrap and a stated assumption about the committed lock and offline builds. |
| Impact | The documented reproduction fails on any other host, so the measurements cannot be re-derived or audited. |
| Recommendation | Resolve bwrap at runtime and state the lock and offline caveat beside the documented commands. |

---

### adr-plan-diagram-wrong-filename

| Field | Detail |
|-------|--------|
| Axis | convention |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/plan.toon:6` |
| Observed | The plan references plan-architecture.mmd, while every other ADR names the diagram architecture.mmd. |
| Expected | The standard diagram filename so a reader or tool finds it. |
| Impact | One ADR out of twenty breaks the only diagram-naming convention the tree has. |
| Recommendation | Rename the file and update the reference, keeping create-sequence.mmd as the genuine second diagram. |

---

### tasks-completion-state-unmarked

| Field | Detail |
|-------|--------|
| Axis | convention |
| Severity | LOW |
| Location | `code://adrs/2026-11-07-devshell-environment/tasks.md:5` |
| Observed | Every task heading lacks a completion marker and the toon titles carry none, while all eight tasks are done and both toon headers still say draft. |
| Expected | Completed tasks marked in both files and the toon status advanced with the work, as sibling ADRs do. |
| Impact | Neither the tooling nor a human can tell from the ADR which tasks were executed and which were skipped. |
| Recommendation | Mark the completed tasks and set the status to the state the map records. |

---

### adr-readme-index-missing

| Field | Detail |
|-------|--------|
| Axis | convention |
| Severity | LOW |
| Location | `code://adrs/README.md:23` |
| Observed | The README lists eight ADRs by status and omits this one, along with two other recent siblings. |
| Expected | An entry for each ADR under the status its spec declares. |
| Impact | The only directory-level entry point into the ADR log silently omits the newest work. |
| Recommendation | Add this ADR under draft and backfill the missing siblings in the same pass. |

---

