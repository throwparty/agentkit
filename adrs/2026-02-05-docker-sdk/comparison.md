# ADR: Docker SDK for Rust - Comparative Analysis and Recommendation

## Executive Summary

- **Recommended default:** `bollard` for direct Docker API access with full FR coverage, active maintenance, and strong ecosystem.
- **Secondary option:** `docker-wrapper` for CLI-driven tooling where a Docker-compatible CLI is already required.
- **Not recommended:** `rs-docker` (unmaintained, brittle parsing) and `anchor` (high-level orchestration focus, unclear FR7/FR8/FR9 support).

## Comparative Summary

| Crate            | FR Coverage (Core)           | Maintenance               | API Style             | Runtime Dependency | PoC Status | Notes                                                 |
| ---------------- | ---------------------------- | ------------------------- | --------------------- | ------------------ | ---------- | ----------------------------------------------------- |
| `bollard`        | Full (FR2/3/6/7/8/9)         | Active                    | Typed async API       | Docker API socket  | Passed     | Best overall fit for SDK needs.                       |
| `docker-wrapper` | Full (FR2/3/6/7/8/9)         | Active                    | CLI builder async     | Docker CLI in PATH | Passed     | Great for CLI tools; higher latency per command.      |
| `rs-docker`      | Full via PoC (hybrid)        | Stale (last release 2021) | Low-level sync API    | Docker API socket  | Ignored    | Unmaintained; PoC needed direct HTTP for correctness. |
| `anchor`         | Partial (FR2/5/6 documented) | Early-stage 0.1.x         | Orchestration-focused | Docker API socket  | Not run    | Orchestration focus; exec/copy not documented.        |

## Detailed Observations

### `bollard`

- Strong fit for SDK-style integration and long-term maintenance.
- Full FR coverage validated by PoC and test suite.
- Direct API access keeps performance predictable and avoids CLI dependency.

### `docker-wrapper`

- Full FR coverage validated by PoC tests.
- Best when CLI semantics are desired and Docker CLI is already installed.
- Adds process-spawn overhead for each command.

### `rs-docker`

- Functionality exists but crate is unmaintained.
- PoC required direct Docker API calls for exec/copy/pull/create reliability.
- Risky for long-term maintenance or correctness.

### `anchor`

- Useful for declarative cluster workflows and metrics.
- Lacks clear documentation for exec/copy operations (FR7/FR8/FR9).
- Not a direct replacement for a general Docker SDK.

## Recommendation

1. **Adopt `bollard`** as the primary SDK for Docker Engine integration.
1. **Keep `docker-wrapper`** as a secondary option for CLI-driven tooling or environments where Docker CLI compatibility is required.
1. **Exclude `rs-docker` and `anchor`** from primary consideration due to maintenance risk and/or incomplete FR coverage.

## Evidence

- `adrs/2026-02-05-docker-sdk/poc-docker-sdk/review.md`
- `adrs/2026-02-05-docker-sdk/poc-docker-sdk/review-docker-wrapper.md`
- `adrs/2026-02-05-docker-sdk/poc-docker-sdk/review-rs-docker.md`
- `adrs/2026-02-05-docker-sdk/poc-docker-sdk/review-anchor.md`
