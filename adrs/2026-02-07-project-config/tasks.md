# ADR: Project Configuration - Implementation Tasks

This document outlines the implementation tasks for the `project-config` feature, based on the `spec.md` and `plan.md` documents. Each task is designed to be independently implementable with clear acceptance criteria and test requirements.

## 1. Core Configuration Structures

### TASK-001: Define `Config` struct

**Status:** Completed ✅
**Description:** Define the `Config` struct in `src/config.rs` (or similar location) to hold the merged configuration values. This struct will contain `project.slug`, `docker.image`, and `docker.setup-command`.

**Acceptance Criteria:**

- A `Config` struct is defined with fields for `project_slug` (String), `docker_image` (String), and `docker_setup_command` (String).
- The struct is derivable with `serde::Deserialize` and `serde::Serialize` for TOML parsing and serialization.
- Default values or optional fields are considered for future expansion, but for now, these fields are mandatory.

**Dependencies:** None
**Estimated Effort:** Small
**Test Requirements:** Unit tests for struct instantiation and field access.
**References:**

- `spec.md`: F4
- `plan.md`: 3.1.1 Config Struct

### TASK-002: Define `ConfigError` enum

**Status:** Completed ✅
**Description:** Define a custom error enum, `ConfigError`, to handle various configuration-related errors such as file not found, parsing errors, and missing required keys.

**Acceptance Criteria:**

- A `ConfigError` enum is defined in `src/config.rs` (or similar location) with variants for:
  - `FileNotFound(PathBuf)`
  - `ParseError(String)` (or a more specific error type from `toml` crate)
  - `MissingRequiredKey(String)`
- The enum implements `std::fmt::Display` and `std::error::Error`.

**Dependencies:** None
**Estimated Effort:** Small
**Test Requirements:** Unit tests for each error variant and their display messages.
**References:**

- `spec.md`: EC1, EC2, EC3
- `plan.md`: 3.1.2 ConfigError Enum

## 2. Configuration Loading Logic

### TASK-003: Create `config_loader` module

**Status:** Completed ✅

**Acceptance Criteria:**

- A new file `src/config_loader.rs` is created.
- The module is declared in `src/lib.rs` or `src/main.rs`.

**Dependencies:** None
**Estimated Effort:** Small
**Test Requirements:** None (module creation only).
**References:**

- `plan.md`: 3.1.3 Config Loader Module

### TASK-004: Implement `config_loader::load_file` function

**Status:** Completed ✅

**Acceptance Criteria:**

- The function correctly reads a TOML file from the given path.
- It uses `serde` and `toml` crates for deserialization.
- It returns `Ok(Config)` on success or `Err(ConfigError::FileNotFound)` if the file does not exist, or `Err(ConfigError::ParseError)` if the file is malformed.
- Handles empty files gracefully (deserializes to a default/empty `Config` if possible, or returns an appropriate error if not).

**Dependencies:** TASK-001, TASK-002, TASK-003
**Estimated Effort:** Medium
**Test Requirements:**

- Unit tests for successful loading of valid TOML.
- Unit tests for `FileNotFound` error.
- Unit tests for `ParseError` with malformed TOML.
- Unit tests for empty TOML files.
  **References:**
- `spec.md`: F1, F2, EC1, EC2, EC4
- `plan.md`: 3.1.3.1 load_file function

### TASK-005: Implement `config_loader::merge` function

**Status:** Completed ✅

**Acceptance Criteria:**

- The function takes two `Config` instances.
- For each field (`project_slug`, `docker_image`, `docker_setup_command`), if the `local` config has a value, it overrides the `base` config's value.
- Returns the merged `Config` struct.

**Dependencies:** TASK-001, TASK-004 (conceptually, for `Config` instances)
**Estimated Effort:** Small
**Test Requirements:**

- Unit tests where local overrides all base values.
- Unit tests where local overrides some base values.
- Unit tests where local has no overrides.
- Unit tests where base has no values (if fields are optional).
  **References:**
- `spec.md`: F3
- `plan.md`: 3.1.3.2 merge function

### TASK-006: Implement `config_loader::load_final` function

**Status:** Completed ✅

**Acceptance Criteria:**

- The function calls `load_file` for `.litterbox.toml`.
- If `.litterbox.toml` is not found, it returns `ConfigError::FileNotFound`.
- It calls `load_file` for `.litterbox.local.toml` (if found, otherwise uses an empty config for merging).
- It calls `merge` to combine the base and local configurations.
- It validates that `project_slug`, `docker_image`, and `docker_setup_command` are present in the final merged `Config`. If any are missing, it returns `ConfigError::MissingRequiredKey`.
- Returns the final `Config` on success.

**Dependencies:** TASK-004, TASK-005
**Estimated Effort:** Medium
**Test Requirements:**

- Integration tests covering the entire data flow diagram.
- Tests for successful loading with both files present.
- Tests for successful loading with only `.litterbox.toml` present.
- Tests for `FileNotFound` for `.litterbox.toml`.
- Tests for `MissingRequiredKey` if any mandatory field is absent after merge.
- Tests for `ParseError` from either file.
  **References:**
- `spec.md`: F1, F2, F3, F4, EC1, EC2, EC3, EC4
- `plan.md`: 3.1.3.3 load_final function, 4. Data Flow Diagram

## 3. Integration and Migration

### TASK-007: Integrate `load_final` into application startup

**Status:** Completed ✅

**Acceptance Criteria:**

- The application successfully starts and retrieves configuration using `load_final()`.
- The sandboxing implementation uses values from the loaded `Config` for `project.slug`, `docker.image`, and `docker.setup-command`.
- If `load_final()` returns an error, the application exits gracefully with an informative error message.

**Dependencies:** TASK-006
**Estimated Effort:** Medium
**Test Requirements:**

- End-to-end tests verifying the application uses the correct configuration values.
- Tests for application startup failure when configuration is invalid or missing.
  **References:**
- `spec.md`: F5
- `plan.md`: 3.2 Integration with Application

### TASK-008: Migrate static configuration values

**Status:** Completed ✅

**Acceptance Criteria:**

- All hardcoded instances of `project.slug`, `docker.image`, and `docker.setup-command` are removed from the sandboxing code.
- The sandboxing code correctly accesses these values from the `Config` struct.
- The sandboxing functionality remains unchanged with the new configuration source.

**Dependencies:** TASK-007
**Estimated Effort:** Medium
**Test Requirements:**

- Existing sandboxing tests continue to pass with the new configuration mechanism.
- Manual verification of sandboxing behavior with different configuration files.
  **References:**
- `spec.md`: AC4
- `plan.md`: 3.2 Integration with Application

## 4. Documentation

### TASK-009: Update project documentation

**Status:** Completed ✅

**Acceptance Criteria:**

- A section in the project's `README.md` or a dedicated `CONFIGURATION.md` file explains the configuration process.
- Examples of `.litterbox.toml` and `.litterbox.local.toml` are provided.
- Instructions on how to set `project.slug`, `docker.image`, and `docker.setup-command` are clear.

**Dependencies:** TASK-008 (after implementation is stable)
**Estimated Effort:** Small
**Test Requirements:** Review of documentation for clarity and accuracy.
**References:** None (general best practice)
