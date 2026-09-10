---

## status: accepted

# ADR: Docker SDK for Rust

## 1. Goals and User Journeys

### Goals

- To provide a robust and idiomatic Rust interface for interacting with a local Docker Engine.
- To enable programmatic creation, deletion, management, and interaction with Docker containers and images from Rust applications.
- To facilitate the development of Rust applications that require comprehensive Docker container management capabilities.
- To define a trait-based interface so multiple Docker client libraries can implement the same operations.

### User Journeys

- **Developer creating a container:** A Rust developer needs to spin up a new Docker container from a specified image (e.g., `ubuntu:latest`, `nginx:stable`) with custom configurations (e.g., container name, environment variables, port mappings) for testing or application deployment.
- **Developer deleting a container:** A Rust developer needs to remove an existing Docker container by its ID or name to clean up resources after a task or test suite has completed.
- **Application managing test environments:** A Rust application automates the setup and teardown of test environments by creating and deleting multiple Docker containers as part of its CI/CD pipeline or local development workflow.
- **Application performing resource cleanup:** A long-running Rust service needs to periodically clean up orphaned or temporary Docker containers to prevent resource exhaustion.
- **Developer pulling images:** A Rust developer wants to programmatically ensure a specific Docker image is available locally before creating a container from it, or to update a local image.
- **Developer executing commands:** A Rust developer needs to run an arbitrary command inside a running container (e.g., `apt update`, `ls /app`, `ps aux`) to inspect its state, perform an action, or retrieve information.
- **Developer copying files:** A Rust developer needs to programmatically transfer configuration files or application binaries from the host into a container, or retrieve logs, results, or generated files from a container to the host.

## 2. Functional Requirements

- **FR1: Connect to Docker Engine:** The SDK MUST be able to establish a connection to a Docker Engine running on the local machine.
- **FR2: Create Container:** The SDK MUST provide a function to create a Docker container. This function MUST accept at least an image name and SHOULD allow for optional configuration parameters such as:
  - Container name
  - Environment variables
  - Port mappings
  - Volume mounts
  - Command to execute
- **FR3: Delete Container:** The SDK MUST provide a function to delete a Docker container. This function MUST accept the container's ID or name as an identifier.
- **FR4: List Containers (SHOULD):** The SDK SHOULD provide a function to list running and/or all Docker containers, returning relevant information such as container ID, name, image, and status.
- **FR5: Start/Stop Containers (SHOULD):** The SDK SHOULD provide functions to start and stop existing Docker containers by their ID or name.
- **FR6: Pull Image:** The SDK MUST provide a function to pull a Docker image from a configured registry (e.g., Docker Hub).
- **FR7: Execute Command in Container:** The SDK MUST provide a function to execute a command inside a running container, capturing its standard output and standard error.
- **FR8: Copy Files into Container:** The SDK MUST provide a function to copy files or directories from the host filesystem into a running container.
- **FR9: Copy Files out of Container:** The SDK MUST provide a function to copy files or directories from a running container to the host filesystem.
- **FR10: Trait-based Interface:** The SDK MUST provide a Rust trait that defines the operations in FR2–FR9 so multiple library implementations can be swapped behind a common interface.

## 3. Non-functional Requirements

- **NFR1: Performance:** All Docker operations (creation, deletion, image pulling, command execution, file copying) MUST be performed efficiently, with minimal overhead introduced by the SDK.
- **NFR2: Reliability:** The SDK MUST handle communication errors with the Docker Engine gracefully, providing clear error messages and recovery mechanisms where appropriate.
- **NFR3: Security:** The SDK MUST adhere to best practices for secure interaction with the Docker daemon, minimizing potential security vulnerabilities, especially when executing commands or copying files.
- **NFR4: Usability:** The API of the SDK MUST be idiomatic Rust, easy to understand, and straightforward to use for Rust developers.
- **NFR5: Maintainability:** The chosen underlying Rust crate(s) (if any) MUST be actively maintained, well-documented, and have a healthy community.
- **NFR6: Compatibility:** The SDK MUST be compatible with commonly used Docker Engine versions.

## 4. Acceptance Criteria

- **AC1: Successful Container Creation:** A Rust program using the SDK can successfully create a Docker container from a specified image and configuration, and the container appears in `docker ps`.
- **AC2: Successful Container Deletion:** A Rust program using the SDK can successfully delete a Docker container by its ID or name, and the container no longer appears in `docker ps -a`.
- **AC3: Error Handling for Invalid Input:** Attempting to create a container with an invalid image name or delete a non-existent container results in a clear and actionable error from the SDK.
- **AC4: API Documentation:** The SDK's public API is comprehensively documented, including examples for common use cases.
- **AC5: Test Coverage:** The SDK (or underlying crate) has adequate test coverage to ensure its functionality and reliability.
- **AC6: Successful Image Pull:** A Rust program can successfully pull a Docker image, and the image appears in `docker images`.
- **AC7: Successful Command Execution:** A Rust program can successfully execute a command inside a running container, capture its standard output/error, and interpret its exit code.
- **AC8: Successful File Copy (into):** A Rust program can successfully copy a file from the host into a specified path within a container, and the file is verifiable inside the container.
- **AC9: Successful File Copy (out of):** A Rust program can successfully copy a file from a specified path within a container to the host filesystem, and the file is verifiable on the host.
- **AC10: Trait-based Implementation:** At least one implementation of the trait-based interface exists and successfully performs the operations in FR2–FR9.

## 5. Edge Cases and Error Handling

- **Docker Engine Unreachable:** The SDK MUST provide a clear error when it cannot connect to the Docker Engine (e.g., Docker daemon not running, incorrect socket path, permission issues).
- **Non-default Socket Path:** The SDK SHOULD allow specifying a non-default Docker socket path (e.g., via `DOCKER_HOST`) and provide a clear error if that path is invalid or unreachable.
- **Invalid Image:** Attempting to create a container with an image that does not exist locally or on a configured registry MUST result in an error. Attempting to pull a non-existent or improperly named image MUST result in an error.
- **Container Not Found:** Attempting to delete, start, stop, execute a command in, or copy files to/from a container using an ID or name that does not correspond to an existing container MUST result in an error.
- **Insufficient Permissions:** If the Rust application lacks the necessary permissions to interact with the Docker daemon, or for file operations within a container, the SDK MUST report a permission-denied error.
- **Resource Constraints:** If the Docker Engine encounters resource limitations (e.g., out of disk space, insufficient memory) during any Docker operation, the SDK should propagate these errors appropriately.
- **Network Issues:** Transient network issues during communication with the Docker Engine or during image pulling should be handled gracefully, potentially with retries or informative error messages.
- **Conflicting Container Names:** Attempting to create a container with a name that is already in use MUST result in an error.
- **Container Still Running (Deletion):** Attempting to delete a running container without force-stopping it first should either fail with an error or offer an option to force deletion.
- **Command Execution Failure:** If a command executed inside a container returns a non-zero exit code, the SDK MUST report this as an error, ideally including the command's standard output and standard error.
- **Invalid Paths for File Copy:** Attempting to copy files to/from invalid source or destination paths (either on the host or within the container) MUST result in an error.
- **File Not Found for Copy:** Attempting to copy a non-existent file from the host into a container or from a container to the host MUST result in an error.
