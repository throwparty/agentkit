---

## status: accepted

# Specification for Git SDK Selection

## 1. Goals

### 1.1 Primary Goal

To select a suitable Git SDK for a Rust project that meets the project's technical and operational requirements, enabling efficient and reliable interaction with **local** Git repositories.

### 1.2 Secondary Goals

- To ensure the chosen SDK is well-maintained and actively supported.
- To minimize the learning curve for developers.
- To provide clear justification for the chosen SDK.
- To avoid writing a custom Git implementation.
- To validate candidate SDKs through a rigorous Proof of Concept (PoC) phase.

## 2. Developer Journeys

### 2.1 Initial Setup

- **Scenario:** A new developer joins the project and needs to set up their environment to work with Git repositories via the SDK.
- **Expected Outcome:** The developer can easily integrate the SDK into their Rust project and perform basic **local** Git operations within a few hours.

### 2.2 Performing Basic Git Operations

- **Scenario:** A developer needs to create a local repository, commit changes, or inspect the status.
- **Expected Outcome:** The SDK provides intuitive and well-documented APIs for common **local** Git operations, allowing developers to perform these tasks efficiently.

### 2.3 Handling Advanced Git Operations

- **Scenario:** A developer needs to perform more complex **local** operations like rebasing local commits, or interacting with Git hooks.
- **Expected Outcome:** The SDK supports these advanced operations, either directly through its API or by providing mechanisms to execute custom Git commands.

### 2.4 Creating Full Tree Snapshots

- **Scenario:** A developer needs to create a commit that represents a complete snapshot of the current working tree, regardless of staged changes.
- **Expected Outcome:** The SDK provides a straightforward and efficient API for generating a commit object from the current state of the working directory, including all tracked files.

### 2.5 Squashing Commits

- **Scenario:** A developer needs to combine multiple related commits into a single, more meaningful commit to streamline history.
- **Expected Outcome:** The SDK provides an intuitive way to select a range of commits and squash them into one, allowing for a new commit message.

### 2.6 Picking Changes (Diff/Patch)

- **Scenario:** A developer needs to extract specific changes from one part of the repository (e.g., a different local branch or an old commit) and apply them to another.
- **Expected Outcome:** The SDK provides functions to generate a patch from a set of changes and apply that patch to a different worktree or local branch, handling potential conflicts gracefully.

### 2.7 Error Handling and Debugging

- **Scenario:** A Git operation fails, or an unexpected state occurs.
- **Expected Outcome:** The SDK provides clear error messages and mechanisms for debugging, helping developers quickly identify and resolve issues.

## 3. Functional Requirements

### 3.1 Core Git Operations

- **FR1.1:** The SDK MUST support initializing a new Git repository.
- **FR1.2:** The SDK MUST support creating commits, including snapshots of the entire working tree, to the local repository.
- **FR1.3:** The SDK MUST support branching operations.
- **FR1.4:** The SDK MUST support checking out branches and specific commits.
- **FR1.5:** The SDK MUST support reading repository history (logs).
- **FR1.6:** The SDK MUST support squashing a range of commits into a single commit.
- **FR1.7:** The SDK MUST support generating diffs (patches) between different local states (e.g., commits, branches, working tree).
- **FR1.8:** The SDK MUST support applying patches to the current working tree or a specified local branch.
- **FR1.9:** The SDK MUST support adding files to the staging area.
- **FR1.10:** The SDK MUST support inspecting the status of the working directory.

## 4. Non-Functional Requirements

### 4.1 Performance

- **NFR1.1:** The SDK SHOULD perform core Git operations with minimal overhead, comparable to or better than direct Git command-line execution for typical use cases.
- **NFR1.2:** The SDK MUST handle large repositories (e.g., >1GB, >100,000 commits) efficiently without excessive memory consumption or performance degradation.

### 4.2 Reliability

- **NFR2.1:** The SDK MUST be stable and robust, handling unexpected scenarios (e.g., corrupted repositories) gracefully.
- **NFR2.2:** The SDK MUST provide consistent and accurate results for all Git operations.

### 4.3 Security

- **NFR3.1:** The SDK SHOULD be regularly audited for security vulnerabilities.

### 4.4 Maintainability

- **NFR4.1:** The SDK MUST have clear and comprehensive documentation.
- **NFR4.2:** The SDK MUST have an active community or commercial support.
- **NFR4.3:** The SDK MUST be compatible with the latest stable Rust versions.
- **NFR4.4:** The SDK SHOULD have a clear release cycle and versioning strategy.

### 4.5 Usability

- **NFR5.1:** The SDK SHOULD provide a user-friendly API that aligns with Rust's idiomatic practices.
- **NFR5.2:** The SDK SHOULD provide clear and informative error messages.

## 5. Acceptance Criteria

### 5.1 Functional Acceptance Criteria

- **AC1.1:** Given a new directory, when the `init` function is called, then a new local Git repository is initialized successfully.
- **AC1.2:** Given a modified file in a local repository, when the `commit` function is called to create a snapshot of the entire tree, then the changes are recorded in the repository history.
- **AC1.3:** Given a range of commits, when the `squash` function is called, then the specified commits are combined into a single new commit with a new message, and the repository history is updated accordingly.
- **AC1.4:** Given two different local states of the repository, when the `diff` function is called, then a patch file is generated that accurately represents the differences between those states.
- **AC1.5:** Given a valid patch file and a target local work tree, when the `apply_patch` function is called, then the changes described in the patch are applied to the target work tree, with conflicts (if any) clearly indicated.
- **AC1.6:** The chosen SDK MUST successfully pass all functional tests defined in the PoC test suite.

### 5.2 Non-Functional Acceptance Criteria

- **AC2.1:** Performance benchmarks for `init`, `commit`, `squash`, `diff`, and `apply_patch` operations on a reference repository (e.g., Linux kernel) MUST be within 10% of the native Git CLI performance.
- **AC2.2:** The SDK's memory usage MUST NOT exceed 2x the native Git CLI for the same operations on large repositories.
- **AC2.3:** The SDK's API documentation MUST cover at least 90% of its public API surface.

## 6. Edge Cases and Error Handling

### 6.1 Repository State

- **EC1.1:** What happens if the local repository is corrupted or in an inconsistent state?
  - **Expected Handling:** The SDK SHOULD detect repository corruption and provide clear error messages, preventing further damage and guiding recovery.
- **EC1.2:** What happens if there are merge conflicts during a merge operation (local only)?
  - **Expected Handling:** The SDK SHOULD report merge conflicts clearly and provide mechanisms for conflict resolution.

### 6.2 Permissions

- **EC2.1:** What happens if the user lacks the necessary permissions for a Git operation (e.g., writing to a protected file)?
  - **Expected Handling:** The SDK SHOULD return a distinct permission-denied error.

### 6.3 Large Files / LFS

- **EC3.1:** How does the SDK handle repositories with large files or Git LFS?
  - **Expected Handling:** The SDK SHOULD ideally support Git LFS or provide clear guidance if it does not.

### 6.4 Empty Repository

- **EC4.1:** What happens when performing operations on an empty or newly initialized repository?
  - **Expected Handling:** Operations should behave predictably and not result in errors unless the operation itself is invalid for an empty repository.

### 6.5 Invalid Input

- **EC5.1:** What happens if invalid input is provided to an SDK function (e.g., non-existent file path)?
  - **Expected Handling:** The SDK MUST validate input and return specific, actionable error messages.

## 7. Proof of Concept (PoC) Phase

### 7.1 Interface Definition

A Rust trait will be defined, outlining the essential Git operations derived from the Functional Requirements, specifically focusing on:

- Reading commit history.
- Creating commits (snapshots of the entire tree).
- Squashing ranges of commits.
- Generating and applying patches.
  This trait will serve as a common interface that all candidate Git SDKs must implement for evaluation.

### 7.2 Test Suite

A comprehensive test suite will be developed against the defined trait. This suite will:

- Cover all relevant functional requirements.
- Utilize a controlled, mock Git repository environment to ensure consistent and reproducible test results.
- Verify correctness, robustness, and adherence to expected Git behavior for each operation.

### 7.3 Candidate Implementations

For each candidate Git SDK identified during the research phase, a concrete implementation of the defined trait will be created. These implementations will serve as the Proof of Concept for each SDK.

### 7.4 Execution and Evaluation

The test suite will be executed against each candidate implementation. The results, including test pass/fail rates, performance metrics (if measurable within the PoC), and ease of implementation, will be key factors in the evaluation and final selection of the Git SDK.

## 8. Environmental Requirements for PoC

To successfully execute the Proof of Concept (PoC) phase for evaluating Git SDKs, the sandbox environment MUST provide the following capabilities:

### 8.1 Rust Toolchain

- **ER1.1:** The sandbox MUST include a complete and functional Rust toolchain (compiler, Cargo) to enable building and running the Rust-based PoC implementations and test suite.

### 8.2 Git SDK Candidate Availability

- **ER1.2:** The sandbox MUST allow for the installation or pre-existence of various Git SDK candidates (e.g., `libgit2` and its Rust bindings like `git2-rs`, or pure Rust implementations) that will be evaluated. This can be achieved via the `container.base-image` or `container.setup-command` as described in the `ROADMAP.md`.
