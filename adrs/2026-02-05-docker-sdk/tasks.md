# ADR: Docker SDK for Rust - Evaluation Tasks

This document outlines the tasks required to evaluate specific existing Rust crates for providing an interface to a Docker Engine, based on the technical plan and the shortlisted crates.

## Task List

### Task 1: Evaluate Crate - `bollard` (COMPLETED - 2026-02-06)

- **Description:** Perform a detailed review of the `bollard` crate against all evaluation criteria outlined in the technical plan (Functional Coverage, API Design & Usability, Documentation Quality, Project Activity & Maintenance, Community Support, Dependencies, License, Reliability & Error Handling, Performance, Compatibility). Additionally, develop a small Rust Proof-of-Concept (PoC) application using this crate to demonstrate its ability to perform the core functional requirements (FR2, FR3, FR6, FR7, FR8, FR9). The PoC should connect to a local Docker Engine.
- **Success Criteria:**
  - A comprehensive review document for `bollard` is created, covering all specified evaluation criteria.
  - The PoC successfully connects to a local Docker Engine.
  - The PoC successfully pulls a specified image (FR6).
  - The PoC successfully creates and deletes a container (FR2, FR3).
  - The PoC successfully executes a simple command inside a container and captures its standard output/error (FR7).
  - The PoC successfully copies a file from the host into a container and from the container to the host (FR8, FR9).
  - The PoC includes basic error handling demonstrations for common failure scenarios (e.g., image not found, container not found).
- **Test Requirements:** The PoC itself serves as a runnable test. It should execute without panics and demonstrate the specified functionalities, printing success messages or relevant output.
- **Dependencies:** None.
- **Estimated Effort:** 2-4 days.
- **Status:** Completed. PoC run successfully with `DOCKER_HOST=unix:///Users/lukecarrier/.lima/docker-arm64/sock/docker.sock`.
- **Specification Reference:** Plan Section 3.1 (Evaluation Methodology - Detailed Review, PoC Implementation), Plan Section 6 (Testing Strategy - PoC Tests).

### Task 2: Evaluate Crate - `rs-docker`

- **Description:** Perform a detailed review of the `rs-docker` crate against all evaluation criteria outlined in the technical plan. Additionally, develop a small Rust Proof-of-Concept (PoC) application using this crate to demonstrate its ability to perform the core functional requirements (FR2, FR3, FR6, FR7, FR8, FR9). The PoC should connect to a local Docker Engine.
- **Success Criteria:**
  - A comprehensive review document for `rs-docker` is created, covering all specified evaluation criteria.
  - The PoC successfully connects to a local Docker Engine.
  - The PoC successfully pulls a specified image (FR6).
  - The PoC successfully creates and deletes a container (FR2, FR3).
  - The PoC successfully executes a simple command inside a container and captures its standard output/error (FR7).
  - The PoC successfully copies a file from the host into a container and from the container to the host (FR8, FR9).
  - The PoC includes basic error handling demonstrations for common failure scenarios (e.g., image not found, container not found).
- **Test Requirements:** The PoC itself serves as a runnable test. It should execute without panics and demonstrate the specified functionalities, printing success messages or relevant output.
- **Dependencies:** None.
- **Estimated Effort:** 2-4 days.
- **Specification Reference:** Plan Section 3.1 (Evaluation Methodology - Detailed Review, PoC Implementation), Plan Section 6 (Testing Strategy - PoC Tests).

### Task 3: Evaluate Crate - `docker-wrapper` (COMPLETED - 2026-02-06)

- **Description:** Perform a detailed review of the `docker-wrapper` crate against all evaluation criteria outlined in the technical plan. Additionally, develop a small Rust Proof-of-Concept (PoC) application using this crate to demonstrate its ability to perform the core functional requirements (FR2, FR3, FR6, FR7, FR8, FR9). The PoC should connect to a local Docker Engine.
- **Success Criteria:**
  - A comprehensive review document for `docker-wrapper` is created, covering all specified evaluation criteria.
  - The PoC successfully connects to a local Docker Engine.
  - The PoC successfully pulls a specified image (FR6).
  - The PoC successfully creates and deletes a container (FR2, FR3).
  - The PoC successfully executes a simple command inside a container and captures its standard output/error (FR7).
  - The PoC successfully copies a file from the host into a container and from the container to the host (FR8, FR9).
  - The PoC includes basic error handling demonstrations for common failure scenarios (e.g., image not found, container not found).
- **Test Requirements:** The PoC itself serves as a runnable test. It should execute without panics and demonstrate the specified functionalities, printing success messages or relevant output.
- **Dependencies:** None.
- **Estimated Effort:** 2-4 days.
- **Status:** Completed. PoC tests run successfully with `DOCKER_HOST=unix:///Users/lukecarrier/.lima/docker-arm64/sock/docker.sock` and `cargo test docker_wrapper::`.
- **Specification Reference:** Plan Section 3.1 (Evaluation Methodology - Detailed Review, PoC Implementation), Plan Section 6 (Testing Strategy - PoC Tests).

### Task 4: Evaluate Crate - `anchor`

- **Description:** Perform a detailed review of the `anchor` crate, with particular attention to its age (not updated in 8 months) and the user's note that it "doesn’t appear to have a stable interface." Focus the review on its functional coverage, API design, and potential issues related to its maintenance status. A full PoC might be deemed unnecessary if the initial review indicates it's not a viable candidate due to instability or lack of maintenance. If it seems viable after initial review, a small PoC should be implemented as well to demonstrate core functional requirements.
- **Success Criteria:**
  - A comprehensive review document for `anchor` is created, highlighting its strengths and weaknesses, and specifically addressing concerns about its age and interface stability.
  - If deemed viable after initial review, a PoC demonstrating core functional requirements (FR2, FR3, FR6, FR7, FR8, FR9) is successfully implemented and executed.
- **Test Requirements:** The PoC (if implemented) serves as a runnable test.
- **Dependencies:** None.
- **Estimated Effort:** 1-2 days.
- **Specification Reference:** Plan Section 3.1 (Evaluation Methodology - Detailed Review, PoC Implementation), Plan Section 6 (Testing Strategy - PoC Tests).

### Task 5: Comparative Analysis and Recommendation (COMPLETED - 2026-02-06)

- **Description:** Compile the detailed review documents and the results from the PoC implementations (from Tasks 1, 2, 3, and 4) into a comprehensive comparative analysis. Provide a final recommendation for the most suitable crate.
- **Success Criteria:** A final document is created that:
  - Compares `bollard`, `rs-docker`, `docker-wrapper`, and `anchor` based on all evaluation criteria.
  - Highlights the strengths and weaknesses of each crate.
  - Provides a clear, justified recommendation for the best crate to proceed with, explaining why it is preferred over others.
  - Includes a summary of the PoC results for each crate (where applicable).
- **Test Requirements:** N/A.
- **Dependencies:** Tasks 1, 2, 3, and 4.
- **Estimated Effort:** 1-2 days.
- **Status:** Completed. Comparative report and recommendation in `adrs/2026-02-05-docker-sdk/comparison.md`.
- **Specification Reference:** Plan Section 3.1 (Evaluation Methodology - Comparative Analysis, Recommendation).
