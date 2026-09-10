---

## status: accepted

# Specification: Project Configuration

## 1. Goals

This specification outlines the design for configuring Litterbox projects. The primary goals are:

- **Define Project Configuration:** Establish a clear and standardized method for users to define project-specific settings for Litterbox.
- **Provide Default Configuration:** Allow projects to define default configuration values that are version-controlled and shared among all contributors.
- **Enable Local Overrides:** Provide a mechanism for individual users to override project-defined configuration locally without affecting the shared project settings.
- **Support Core Sandboxing Needs:** Ensure the configuration system can provide essential values for sandboxing, specifically `project.slug`, `docker.image`, and `docker.setup-command`.
- **Facilitate Migration:** Migrate existing statically assigned configuration values within the sandboxing implementation to use the new dynamic configuration system.
- **Future Extensibility:** Design the configuration system to be easily extensible for additional configuration options in the future.

## 2. User Journeys

### 2.1. New Project Setup

A user initializes a new project and wants to integrate Litterbox.

1. The user creates a `.litterbox.toml` file in the project root.
1. They define essential configuration keys: `project.slug`, `docker.image`, and `docker.setup-command` within this file.
1. Litterbox reads and applies these configurations when operating on the project.

### 2.2. Existing Project Configuration and Local Customization

A user joins an existing project that already has a `.litterbox.toml` file.

1. The user wants to test a different Docker image or change the project ID for their local development environment without committing these changes to the shared repository.
1. They create a `.litterbox.local.toml` file in the project root.
1. They specify their desired overrides (e.g., `docker.image = "my-custom-image:dev"`) in `.litterbox.local.toml`.
1. Litterbox merges the configurations, prioritizing values from `.litterbox.local.toml`, and uses the customized settings for the user's local operations.

### 2.3. Updating Shared Configuration

A project maintainer decides to update the default `docker.image` for the project.

1. The maintainer modifies the `docker.image` value in the `.litterbox.toml` file and commits the change.
1. Other users, upon pulling the latest changes, automatically pick up the new default `docker.image` when running Litterbox, unless they have a specific override in their `.litterbox.local.toml`.

### 2.4. Migrating Static Values

During the implementation of this feature, existing hardcoded configuration values in the sandboxing logic need to be replaced.

1. The development team identifies all statically assigned values related to `project.slug`, `docker.image`, and `docker.setup-command`.
1. These static values are replaced with dynamic lookups from the newly implemented configuration system.
1. The sandboxing implementation now relies entirely on the `.litterbox.toml` (and `.litterbox.local.toml`) for these settings.

## 3. Functional Requirements

- **F1: Configuration File Discovery:** The system SHALL search for a file named `.litterbox.toml` in the root directory of the current project.
- **F2: Local Override File Discovery:** The system SHALL search for a file named `.litterbox.local.toml` in the root directory of the current project.
- **F3: Configuration Merging:** If both `.litterbox.toml` and `.litterbox.local.toml` are found, the system SHALL merge their contents. Values defined in `.litterbox.local.toml` SHALL take precedence over (override) identical keys in `.litterbox.toml`.
- **F4: Required Configuration Keys:** The final merged configuration SHALL contain the following keys:
  - `project.slug` (String): A unique identifier for the project, used in naming conventions (e.g., container names).
  - `docker.image` (String): The Docker image to be used for the project's sandbox environment.
  - `docker.setup-command` (String): A command or script to be executed to set up the environment within the Docker container.
- **F5: Configuration Access:** The sandboxing implementation SHALL be able to retrieve the values of `project.slug`, `docker.image`, and `docker.setup-command` from the merged configuration.
- **F6: Extensibility:** The configuration parsing logic SHALL be designed to gracefully handle and ignore unknown keys, allowing for future additions to the configuration schema without requiring changes to existing parsing logic.

## 4. Non-functional Requirements

- **N1: Performance:** The process of reading, parsing, and merging configuration files SHALL be efficient, introducing negligible overhead to Litterbox's startup or operation.
- **N2: Maintainability:** The configuration parsing and merging logic SHALL be modular, well-documented, and easy to understand and modify.
- **N3: Readability:** The configuration files SHALL use the TOML format, ensuring human-readability and ease of editing.
- **N4: Robustness:** The system SHALL be resilient to malformed configuration files and missing optional files, providing clear error messages without crashing.

## 5. Acceptance Criteria

- **AC1: Base Configuration Loading:** Given a project with only a `.litterbox.toml` file containing valid `project.slug`, `docker.image`, and `docker.setup-command` values, Litterbox SHALL successfully load and use these values.
- **AC2: Local Override Functionality:** Given a project with both `.litterbox.toml` and `.litterbox.local.toml`, where `.litterbox.local.toml` specifies a different `docker.image` than `.litterbox.toml`, Litterbox SHALL use the `docker.image` value from `.litterbox.local.toml` and other values from `.litterbox.toml`.
- **AC3: Missing Required Keys (Error Handling):** If, after merging `.litterbox.toml` and `.litterbox.local.toml`, any of `project.slug`, `docker.image`, or `docker.setup-command` are missing, Litterbox SHALL terminate with a clear error message indicating which key(s) are missing.
- **AC4: Migration Verification:** All hardcoded instances of `project.slug`, `docker.image`, and `docker.setup-command` within the sandboxing implementation SHALL be removed and replaced with references to the new configuration system.
- **AC5: Unknown Key Handling:** If `.litterbox.toml` or `.litterbox.local.toml` contain keys not currently defined in the schema, Litterbox SHALL parse the file without error and ignore the unknown keys.

## 6. Edge Cases and Error Handling

- **EC1: Missing `.litterbox.toml`:** If `.litterbox.toml` is not found, the system SHALL report an error indicating that a configuration file is required.
- **EC2: Invalid TOML Format:** If either `.litterbox.toml` or `.litterbox.local.toml` contains invalid TOML syntax, the system SHALL report a parsing error, ideally indicating the file and line number of the error, and terminate gracefully.
- **EC3: Empty Configuration Files:** Empty `.litterbox.toml` or `.litterbox.local.toml` files SHALL be treated as valid but empty configuration sources, effectively contributing no values to the merge.
- **EC4: Type Mismatch in Merged Keys:** If a key exists in both `.litterbox.toml` and `.litterbox.local.toml` but with incompatible data types (e.g., `docker.image` is a string in one and an integer in the other), the system SHALL report a type mismatch error and prioritize the type from `.litterbox.local.toml` if possible, or terminate with an error if not. (This assumes TOML parsing libraries might handle some coercion, but explicit error is safer).
- **EC5: Non-string values for required keys:** If `project.slug`, `docker.image`, or `docker.setup-command` are present but are not of type string, the system SHALL report a type error.
- **EC6: File Permissions:** If the system lacks read permissions for `.litterbox.toml` or `.litterbox.local.toml`, it SHALL report a permission error and terminate.
