---

## status: accepted

# Feature Specification: Port Forwarding for Sandbox Containers

## 1. Goals

- Enable developers to access services running inside sandbox containers from their local development workstation.
- Support multiple concurrent sandboxes, each potentially running services on the same internal container ports.
- Support multiple servers or services running on an individual container, requiring flexible port mapping.
- Automatically assign unused host ports to avoid conflicts.
- Provide a seamless development experience for interacting with sandboxed applications.

## 2. User Journeys

### User Journey 1: Starting a new sandbox with port forwarding

1. Developer creates a new sandbox, specifying internal container ports via project-level configuration.
1. The system automatically assigns available, unused host ports for each specified container port.
1. The system exposes the assigned host port numbers as environment variables within the container (e.g., `LITTERBOX_FWD_PORT_MY_SERVICE=32768`, where `MY_SERVICE` is the slugified service name from the project configuration).
1. The system informs the developer of the assigned host ports, including a mapping of container ports to host ports in the `sandbox-create` tool response.
1. Developer accesses the services running in the sandbox using the assigned host ports (e.g., `localhost:32768` for a web server).

### User Journey 2: Accessing an existing sandbox with port forwarding

1. Developer lists active sandboxes and their associated port mappings.
1. Developer connects to a service in a running sandbox using the previously assigned host port.

### User Journey 4: Retrieving forwarded ports after creation context is gone

1. Developer asks for the forwarded ports of a running sandbox after losing the original `sandbox-create` response.
1. The system returns the forwarded port mappings for that sandbox on demand.

### User Journey 3: Handling port conflicts

1. Developer attempts to start a sandbox, but a requested host port is already in use.
1. The system detects the conflict and either:
   a. Automatically re-assigns a new unused host port (preferred).
   b. Informs the user of the conflict and suggests alternative ports or actions.

## 3. Functional Requirements

- FR1: Port Forwarding Configuration: The system SHALL allow users to specify which internal container ports should be forwarded, through project-level configuration that supports multiple ports per container.
- FR2: Automatic Host Port Assignment: The system SHALL automatically assign an available, unused host port for each forwarded container port within the default range 3000-8000.
- FR3: Multiple Sandbox Support: The system SHALL support port forwarding for multiple concurrently running sandboxes without port conflicts.
- FR4: Port Information Retrieval: The system SHALL return the assigned forwarded ports for a sandbox via the `sandbox-create` tool response.
- FR5: Connection Establishment: The system SHALL configure the sandboxing mechanism to forward the assigned host ports to the corresponding container ports.
- FR6: Environment Variable Exposure: The system SHALL expose the randomly assigned host port numbers as environment variables within the container for each forwarded port.
- FR7: `sandbox-create` Tool Response: The `sandbox-create` tool SHALL include a mapping of configured container ports to their assigned host ports in its response.
- FR8: `sandbox-ports` Tool: The system SHALL expose a `sandbox-ports` tool that returns forwarded port mappings for a named sandbox.

## 4. Non-Functional Requirements

- NFR3: Security: Port forwarding SHALL only expose explicitly configured ports and not inadvertently open other container ports to the host.
- NFR4: Usability: The process of configuring and accessing forwarded ports SHALL be intuitive and well-documented.
- NFR5: Scalability: The port assignment mechanism SHALL efficiently handle a large number of sandboxes and port forwarding requests.

## 5. Acceptance Criteria

- AC1: A user can start a sandbox, specify container port 8080, and successfully access a web server running on `localhost:<assigned_port>`.
- AC2: Two different sandboxes can be started, both exposing container port 8080, and each is assigned a unique host port, allowing both web servers to be accessed concurrently.
- AC3: The `sandbox-create` tool response includes the assigned forwarded ports for the sandbox.
- AC4: If a previously assigned host port becomes unavailable (e.g., another process takes it), the system either re-assigns a new port or clearly communicates the issue to the user.
- AC5: The port assignment logic correctly identifies and utilizes unused ports within the default range 3000-8000.
- AC6: When a container is started, environment variables are present with the forwarded host port for each configured container port (e.g., `LITTERBOX_FWD_PORT_SERVICE_A=32768`).
- AC7: The `sandbox-create` tool returns a clear mapping of container ports to host ports upon successful sandbox creation.
- AC8: The `sandbox-ports` tool returns the same forwarded port mappings for a sandbox as originally returned by `sandbox-create`.

## 6. Edge Cases and Error Handling

- EC1: No available host ports: If the system cannot find an unused host port within the default range 3000-8000, it SHALL inform the user and suggest actions (e.g., free up ports).

- EC2: Invalid container port: If a user specifies a non-numeric, out-of-range, or zero container port, the system SHALL reject the request with an appropriate error message.

- EC3: Sandbox termination: When a sandbox is terminated, its assigned host ports SHALL be released and made available for other sandboxes.

- EC4: Unknown sandbox: If a user requests port mappings for a sandbox that does not exist, the system SHALL return a clear error.

- EC5: Long-running services: Port forwarding SHALL remain active and stable for long-running services within the sandbox.

- EC7: Environment variable name conflicts: If, after slugification, multiple service names result in the same environment variable name, the system SHALL report an error and require the user to resolve the conflict in the project configuration.
