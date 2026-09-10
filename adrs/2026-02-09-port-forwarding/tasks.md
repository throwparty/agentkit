# Tasks for Port Forwarding Feature

This document outlines the implementable tasks for the Port Forwarding feature, based on the specification (`spec.md`) and technical plan (`plan.md`). Each task is designed to be an independently implementable unit of work, with clear success criteria and test requirements.

## 1. Configuration Module (`src/config.rs`, `src/config_loader.rs`)

### [x] Task 1.1: Define `ForwardedPort` and `PortsConfig` Structs

**Description**: Define the `ForwardedPort` struct (containing `name: String` and `target: u16`) and the `PortsConfig` struct (containing `ports: Vec<ForwardedPort>`) in `src/config.rs`.
**Success Criteria**: New structs are defined and compile without errors.
**Test Requirements**: Unit tests for struct instantiation and field access.
**References**: `spec.md` (Functional Requirements 3, 4, 5), `plan.md` (Section 3.1)

### [x] Task 1.2: Extend Main `Config` Struct

**Description**: Add a `ports: Option<PortsConfig>` field to the main `Config` struct in `src/config.rs`.
**Success Criteria**: `Config` struct updated and compiles. The `ports` field is optional.
**Test Requirements**: Unit tests to ensure `Config` can be deserialized with and without the `ports` field.
**References**: `plan.md` (Section 3.1)

### [x] Task 1.3: Update `config_loader` for Port Configuration Parsing

**Description**: Modify `src/config_loader.rs` to parse the `[[ports]]` table from `.litterbox.toml` and `.litterbox.local.toml` into the `PortsConfig` struct. This includes slugifying the `name` field for environment variable generation and detecting conflicts in slugified names.
**Success Criteria**: `config_loader` successfully parses valid port configurations and reports errors for invalid ones (e.g., duplicate slugified names, invalid port numbers).
**Test Requirements**: \* Unit tests for `config_loader` to parse valid `PortsConfig`. \* Unit tests for `config_loader` to correctly slugify names (e.g., "Web Server" -> "WEB_SERVER"). \* Unit tests for `config_loader` to detect and report errors on duplicate slugified names. \* Unit tests for `config_loader` to detect and report errors on invalid port numbers (ee.g. 0 or > 65535).
**References**: `spec.md` (Functional Requirement 4), `plan.md` (Section 3.1)

## 2. Sandbox Module (`src/domain.rs`, `src/sandbox/mod.rs`, `src/compute/mod.rs`)

### [x] Task 2.1: Extend `SandboxConfig` and `SandboxMetadata`

**Description**: \* Extend `SandboxConfig` (defined in `src/domain.rs`) to include the parsed `PortsConfig`. \* Extend `SandboxMetadata` (defined in `src/domain.rs`) to store the generated host port forwarding information and environment variable names.
**Success Criteria**: `SandboxConfig` and `SandboxMetadata` structs are updated and compile.
**Test Requirements**: Unit tests for instantiation and field access of updated structs.
**References**: `plan.md` (Section 3.2)

### [x] Task 2.2: Extend `ContainerSpec`

**Description**: In `src/compute/mod.rs`, modify the `ContainerSpec` struct to include `pub env: Vec<String>` for environment variables and `pub port_bindings: HashMap<String, Vec<bollard::models::PortBinding>>` for port forwarding.
**Success Criteria**: `ContainerSpec` is updated and compiles.
**Test Requirements**: Unit tests for `ContainerSpec` instantiation with and without the new fields.
**References**: `plan.md` (Section 3.3)

### [x] Task 2.3: Implement Dynamic Port Allocation and Env Var Generation

**Description**: Implement the logic within `DockerSandboxProvider::create` in `src/sandbox/mod.rs` to: \* Access the `PortsConfig` from `SandboxConfig`. \* For each `ForwardedPort`, dynamically find an available host port within the default range of 3000-8000. \* Generate the `LITTERBOX_FWD_PORT_<SLUGIFIED_NAME>` environment variables. \* Handle concurrency conflicts during port allocation using a simple retry mechanism with a small back-off.
**Success Criteria**: Host ports are successfully allocated, environment variables are generated, and a list of `bollard::models::PortBinding` is created. Retry mechanism for port allocation is functional.
**Test Requirements**: \* Unit tests for host port allocation logic, ensuring unique and available ports are selected from the default range. \* Unit tests for environment variable generation. \* Unit tests for the retry mechanism in port allocation.
**References**: `spec.md` (Functional Requirements 1, 5), `plan.md` (Section 3.2)

### [x] Task 2.4: Modify `DockerCompute::create_container` for Port Bindings and Env Vars

**Description**: Update `DockerCompute::create_container` in `src/compute/mod.rs` to: \* Accept the extended `ContainerSpec`. \* Populate `bollard::models::ContainerCreateBody.Env` from `spec.env`. \* Construct `bollard::models::HostConfig` and populate `HostConfig.PortBindings` from `spec.port_bindings`. Set `HostIp` to "0.0.0.0" and `HostPort` to the allocated host port (as a `String`) within `PortBinding`.
**Success Criteria**: Docker containers are created with correct port forwarding rules and environment variables.
**Test Requirements**: \* Integration tests to verify Docker containers are created with the specified port bindings and environment variables. \* Note: `bollard`-specific errors during container creation are expected to propagate up the call stack for handling by `DockerSandboxProvider`.
**References**: `spec.md` (Functional Requirements 2, 5), `plan.md` (Section 3.3)

### [x] Task 2.5: Populate `SandboxMetadata`

**Description**: Populate the `SandboxMetadata` with the generated host port forwarding information (container port, host port, slugified name) before returning it from `DockerSandboxProvider::create`.
**Success Criteria**: `SandboxMetadata` contains accurate port forwarding data.
**Test Requirements**: Unit tests to verify `SandboxMetadata` is correctly populated.
**References**: `plan.md` (Section 3.2)

## 3. Model Control Protocol Module (`src/mcp.rs`)

### [x] Task 3.1: Update `sandbox-create` Tool Response

**Description**: Modify the `sandbox_create` tool in `src/mcp.rs` to return the enriched `SandboxMetadata` (which now includes forwarded port information) back to the client.
**Success Criteria**: The `sandbox-create` tool's response contains the complete forwarding port mappings.
**Test Requirements**: Integration tests to verify the `sandbox-create` command's output includes the correct forwarding port mappings and environment variables.
**References**: `spec.md` (Functional Requirement 6), `plan.md` (Section 3.4)

### [x] Task 3.2: Add `sandbox-ports` Tool

**Description**: Add a new MCP tool `sandbox-ports` that returns forwarded port mappings for a named sandbox. It should inspect the container to derive `forwarded_ports` and return a structured response (name + mappings). Return a clear error if the sandbox is missing.
**Success Criteria**: `sandbox-ports` returns accurate forwarded port mappings for an existing sandbox and a clear error for unknown sandbox.
**Test Requirements**: Integration tests for successful lookup and missing sandbox error.
**References**: `spec.md` (FR8, AC8, EC4), `plan.md` (Section 3.5)

## 4. Testing and Error Handling

### [x] Task 4.1: End-to-End Integration Tests

**Description**: Develop comprehensive integration tests that cover the entire flow from configuration parsing to sandbox creation and verification of forwarded ports and environment variables.
**Success Criteria**: All integration tests pass, demonstrating end-to-end functionality.
**Test Requirements**: Test scenarios with multiple sandboxes, various port configurations, and invalid inputs.
**References**: `plan.md` (Section 5)

### [x] Task 4.2: Edge Cases and Validation Tests

**Description**: Implement tests for identified edge cases, such as invalid port configurations, no available host ports (simulated), and duplicate slugified names.
**Success Criteria**: Edge case tests pass, and error handling mechanisms are triggered correctly.
**Test Requirements**: Unit and integration tests for all edge cases mentioned in `spec.md` and `plan.md`.
**References**: `spec.md` (Edge Cases), `plan.md` (Section 5)

### [x] Task 4.3: Error Handling Implementation and Testing

**Description**: Implement robust error handling throughout the port forwarding mechanism. Ensure that `bollard`-specific errors propagate up, and the retry mechanism for port allocation failures is correctly implemented and tested.
**Success Criteria**: The system gracefully handles errors, provides informative messages, and the retry mechanism for port allocation works as expected.
**Test Requirements**: Unit and integration tests for error propagation and the retry mechanism.
**References**: `spec.md` (Edge Cases), `plan.md` (Section 5)

## 5. Documentation and Future Considerations

### [x] Task 5.1: Update Documentation

**Description**: Update relevant user-facing and developer documentation to reflect the new port forwarding configuration options and usage.
**Success Criteria**: Documentation is clear, accurate, and complete.
**Test Requirements**: Manual review of documentation.
**References**: N/A

### [x] Task 5.2: Future Consideration - Customizable Port Range

**Description**: Document the ability for users to customize the dynamic host port range as a future consideration for the project.
**Success Criteria**: The future consideration is clearly articulated in the documentation.
**Test Requirements**: N/A
**References**: User feedback.
