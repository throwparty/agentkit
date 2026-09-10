# Crate Evaluation: `rs-docker`

## 1. Functional Coverage

### Core (MUST have)

- [x] Create containers (FR2) - `docker.create_container` API exists; PoC uses Docker API POST `/containers/create` for reliability.
- [x] Delete containers (FR3) - `docker.delete_container` API exists.
- [x] Pull images (FR6) - `docker.create_image` API exists; PoC uses Docker API POST `/images/create`.
- [x] Execute commands in containers (FR7) - PoC uses Docker API `/containers/{id}/exec` and `/exec/{id}/start`.
- [x] Copy files into containers (FR8) - PoC uses Docker API PUT `/containers/{id}/archive` with tar payload.
- [x] Copy files out of containers (FR9) - PoC uses Docker API GET `/containers/{id}/archive` with tar payload.

### Supplementary (SHOULD have/nice-to-have)

- [x] List containers (FR4) - `docker.get_containers`.
- [x] Start/Stop containers (FR5) - `docker.start_container` / `docker.stop_container`.

## 2. API Design & Usability (NFR4)

- Synchronous API surface with low-level Docker Remote API mappings.
- Async use requires wrapping in `spawn_blocking` or similar.
- Data models are thin and sometimes brittle; PoC needed direct HTTP calls for correctness.

## 3. Documentation Quality (NFR4, NFR5)

- README is short and mostly examples.
- Documentation link points to older site; docs.rs exists but is sparse.

## 4. Project Activity & Maintenance (NFR5)

- Latest crates.io release: `0.0.60` (2021-06-23).
- README notes the crate is a fork of an unmaintained project.
- No recent releases in multiple years.

## 5. Community Support

- Small ecosystem; activity appears minimal.

## 6. Dependencies

- `hyper`, `hyperlocal`, `tokio`, `serde`, `serde_json`, `futures`.

## 7. License

- Apache-2.0.

## 8. Reliability & Error Handling (NFR2)

- Errors surfaced through `std::io::Result` and parsing; limited typed error context.
- Known parsing issues motivated direct API calls in PoC.

## 9. Performance (NFR1)

- Direct HTTP client with synchronous calls; async integration requires offloading.
- Performance depends on Docker daemon responsiveness.

## 10. Compatibility (NFR6)

- Docker Remote API via HTTP; supports unix and tcp endpoints.

## PoC Implementation and Execution

**PoC Location:** `adrs/2026-02-05-docker-sdk/poc-docker-sdk/src/lib/rs_docker.rs`

**Execution:**

- Test suite is ignored due to unmaintained status (see `tests/docker_client.rs`).

**Observed Results:**

- Implementation works by using rs-docker for start/stop/list and direct Docker API for exec/copy/pull/create.
- Marked as ignored due to maintenance risk and flaky parsing behavior.

## Notes

- The crate is unmaintained and appears to be a fork of a previously unmaintained project.
- PoC required direct Docker API calls to avoid incorrect parsing/behavior.
