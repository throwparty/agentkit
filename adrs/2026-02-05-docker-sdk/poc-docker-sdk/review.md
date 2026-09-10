# Crate Evaluation: `bollard`

## 1. Functional Coverage

### Core (MUST have)

- [x] Create containers (FR2) - `Docker::create_container` with `ContainerCreateBody` and `CreateContainerOptionsBuilder`.
- [x] Delete containers (FR3) - `Docker::remove_container` with `RemoveContainerOptionsBuilder`.
- [x] Pull images (FR6) - `Docker::create_image` with `CreateImageOptionsBuilder`.
- [x] Execute commands in containers (FR7) - `Docker::create_exec` + `Docker::start_exec` returning `StartExecResults::Attached`.
- [x] Copy files into containers (FR8) - `Docker::upload_to_container` with `UploadToContainerOptionsBuilder`.
- [x] Copy files out of containers (FR9) - `Docker::download_from_container` with `DownloadFromContainerOptionsBuilder`.

### Supplementary (SHOULD have/nice-to-have)

- [x] List containers (FR4) - `Docker::list_containers` with `ListContainersOptionsBuilder`.
- [x] Start/Stop containers (FR5) - `Docker::start_container` / `Docker::stop_container` with `StopContainerOptionsBuilder`.

## 2. API Design & Usability (NFR4)

- Idiomatic Rust: Strongly typed request/response models, builder patterns for options.
- Ease of Use: Straightforward when familiar with async Rust; requires understanding of query parameter builders and models.
- Modern Rust features: Uses `async/await`, `Stream`/`StreamExt`, `Result` error handling.
- Examples: docs.rs and crate examples demonstrate common workflows (exec, build, stats, etc.).

## 3. Documentation Quality (NFR4, NFR5)

- `rustdoc` completeness and organization: Comprehensive docs with examples in `docs.rs`.
- `README.md` and/or external guides: `README` covers installation, features, and usage patterns.
- Up-to-date documentation: Versioned with current releases; API version noted in docs.

## 4. Project Activity & Maintenance (NFR5)

- Last commit date: 2026-02-01 (GitHub `pushedAt`).
- Frequency of releases: Recent; `bollard` v0.20.1 on crates.io (current in this evaluation).
- Open issues/PRs and maintainer responsiveness: Active project with ongoing changes.
- Bus factor: Multiple contributors; not a single-maintainer project.

## 5. Community Support

- GitHub stars/forks: 1202 stars / 164 forks (as of 2026-02-06).
- Forums/discussions: Widely referenced in Rust Docker integrations.

## 6. Dependencies

- Minimal and well-chosen dependencies: `tokio`, `hyper`, `hyper-util`, `serde` + stubs generated from Docker OpenAPI.
- Security vulnerabilities in transitive dependencies: None identified in this evaluation; would require advisory scan for production.
- Dependency conflicts: `tokio` version alignment is the main consideration.

## 7. License

- Apache-2.0 (compatible with typical project licensing).

## 8. Reliability & Error Handling (NFR2)

- Handling of Docker Engine communication errors: Explicit error types from `bollard::errors::Error` and HTTP status handling.
- Clear, actionable error types: Error messages include HTTP status and Docker API error strings.

## 9. Performance (NFR1)

- Built on `hyper` and async streams; no evident bottlenecks in normal API usage. Performance depends on Docker daemon responsiveness and network I/O.

## 10. Compatibility (NFR6)

- Docker Engine API: Uses v1.52 schema with API version negotiation support.
- Updates for new Docker features: Active maintenance suggests timely updates.

## PoC Implementation and Execution

**PoC Location:** `adrs/2026-02-05-docker-sdk/bollard_evaluation/src/main.rs`

**Dependencies:**

- `bollard = "0.20.1"`
- `tokio = { version = "1", features = ["full"] }`
- `tar = "0.4"`
- `futures = "0.3"`
- `bytes = "1"`

**Execution:**

- `cargo test` succeeded (compilation only, no unit tests defined).
- `cargo run` succeeded against Lima Docker socket using:
  - `DOCKER_HOST=unix:///Users/lukecarrier/.lima/docker-arm64/sock/docker.sock`

**Observed Results:**

- Image pull: `alpine:latest` pulled successfully.
- Container create/start/list: container created and listed as running.
- Exec command: `cat /container_file.txt` output captured and validated.
- Copy in/out: file uploaded via archive and downloaded successfully, content verified.
- Stop/delete: container stopped and removed successfully.
- Cleanup: image removed and host files cleaned.
- Error handling: deletion of non-existent container returned 404 as expected.

## Notes

- API surface has shifted from `container::CreateContainerOptions` to `query_parameters::CreateContainerOptionsBuilder` and `models::ContainerCreateBody` in recent versions.
- For archive operations, use `upload_to_container` and `download_from_container` rather than older archive methods.
