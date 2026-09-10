# Crate Evaluation: `docker-wrapper`

## 1. Functional Coverage

### Core (MUST have)

- [x] Create containers (FR2) - `CreateCommand::new(image).name(...).cmd(...).run()`.
- [x] Delete containers (FR3) - `RmCommand::new(container).force().execute()`.
- [x] Pull images (FR6) - `PullCommand::new(image).execute()`.
- [x] Execute commands in containers (FR7) - `ExecCommand::new(container, cmd).execute()`.
- [x] Copy files into containers (FR8) - `CpCommand::from_host(path).to_container(container, path).run()`.
- [x] Copy files out of containers (FR9) - `CpCommand::from_container(container, path).to_host(path).run()`.

### Supplementary (SHOULD have/nice-to-have)

- [x] List containers (FR4) - `PsCommand::new().all().execute()` or `docker ps --format "{{json .}}"`.
- [x] Start/Stop containers (FR5) - `StartCommand::new(container).execute()` / `StopCommand::new(container).execute()`.

## 2. API Design & Usability (NFR4)

- Builder pattern mirrors Docker CLI, with typed outputs for most commands.
- Async-first API via `tokio` and `DockerCommand::execute()`.
- CLI dependency: requires Docker-compatible CLI in PATH; uses process spawn for each command.
- Escape hatches available via raw `arg`/`args` on `DockerCommand`.

## 3. Documentation Quality (NFR4, NFR5)

- docs.rs has comprehensive command coverage and examples.
- README and examples cover common workflows and feature flags (compose, swarm, manifest, templates).

## 4. Project Activity & Maintenance (NFR5)

- crates.io version: `0.10.2`.
- Rust version requirement: `1.89.0` (from crate metadata).
- GitHub activity metrics not verified in this environment.

## 5. Community Support

- GitHub stars/forks/issues not verified in this environment.

## 6. Dependencies

- Core deps: `tokio`, `serde`, `serde_json`, `tracing`, `which`, `thiserror`.
- Optional `reqwest` for templates.
- No direct Docker API dependency; leverages external CLI.

## 7. License

- MIT OR Apache-2.0.

## 8. Reliability & Error Handling (NFR2)

- `CommandExecutor` returns structured errors with exit code, stdout, stderr.
- Optional per-command timeouts.
- Reliability depends on Docker CLI availability and behavior.

## 9. Performance (NFR1)

- Process spawn per command; higher latency than direct API clients.
- Suitable for CLI tools and lower-frequency operations.

## 10. Compatibility (NFR6)

- Works with Docker CLI; supports compatible runtimes (Podman, Colima) via platform/runtime abstraction.
- Honors environment like `DOCKER_HOST` through the underlying CLI.

## PoC Implementation and Execution

**PoC Location:** `adrs/2026-02-05-docker-sdk/poc-docker-sdk`

**Execution:**

- `DOCKER_HOST=unix:///Users/lukecarrier/.lima/docker-arm64/sock/docker.sock cargo test docker_wrapper::`

**Observed Results:**

- Image pull: `busybox:latest` pulled successfully.
- Container create/start/list: container created and listed as running.
- Exec command: `echo -n hello` output captured and validated.
- Copy in/out: file uploaded and downloaded successfully, content verified.
- Stop/remove: container stopped and removed successfully.
- Error handling: removal of non-existent container returned error as expected.

## Notes

- Strong choice for tools that already rely on Docker CLI semantics.
- CLI dependency is the primary tradeoff versus direct Docker API crates.
