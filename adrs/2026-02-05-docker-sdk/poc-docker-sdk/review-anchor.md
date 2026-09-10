# Crate Evaluation: `anchor`

## 1. Functional Coverage

### Core (MUST have)

- [x] Create containers (FR2) - `build_container` (from README).
- [x] Start/stop containers (FR5) - `start_container` / `get_container_status` (from README).
- [x] Pull images (FR6) - `pull_image` (from README).
- [ ] Delete containers (FR3) - not documented in README; likely exists but not confirmed.
- [ ] Execute commands in containers (FR7) - not documented.
- [ ] Copy files into containers (FR8) - not documented.
- [ ] Copy files out of containers (FR9) - not documented.

### Supplementary (SHOULD have/nice-to-have)

- [x] Container metrics/health (beyond FRs) - `get_container_metrics` and health reporting in README.

## 2. API Design & Usability (NFR4)

- High-level, opinionated API focused on declarative cluster management.
- Async-first API using `tokio`.
- Uses `bollard` internally (per README), so likely Docker API-based.
- Surface area seems tuned for orchestrating named containers rather than low-level Docker operations.

## 3. Documentation Quality (NFR4, NFR5)

- README provides a guided walkthrough, examples, and core concepts.
- docs.rs available, but detailed coverage of exec/copy APIs not visible in README.

## 4. Project Activity & Maintenance (NFR5)

- crates.io latest: `0.1.3`, published 2025-06-23.
- GitHub last push: 2025-06-23.
- Early-stage API (`0.1.x`) suggests potential instability.

## 5. Community Support

- GitHub stars/forks: 0/0 (as of 2025-06-23).
- Small community footprint.

## 6. Dependencies

- Core: `bollard`, `tokio`, `chrono`.
- Optional: `aws-sdk-ecr`, `base64` for ECR support.

## 7. License

- MIT OR Apache-2.0 (crates.io metadata).

## 8. Reliability & Error Handling (NFR2)

- Custom error types (`AnchorError`) documented in README.
- Error handling appears structured, but deeper behavior not reviewed.

## 9. Performance (NFR1)

- Docker API-backed via `bollard`; performance tied to Docker daemon and network I/O.
- Additional overhead for cluster-level orchestration features.

## 10. Compatibility (NFR6)

- Cross-platform support claimed (Linux, macOS, Windows).
- MSRV 1.70+ in README.

## PoC Implementation and Execution

**Decision:** No PoC implemented.

**Rationale:** The README and surface documentation focus on cluster orchestration and metrics. Core FRs around exec and file copy are not documented, and the crate is early-stage with minimal community adoption. A PoC would be premature without evidence of FR7/FR8/FR9 support.

## Notes

- This crate is positioned more as a high-level orchestration layer than a general Docker SDK.
- If needed, a deeper API audit is required to confirm exec/copy support before investing in a PoC.
