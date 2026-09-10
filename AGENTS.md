# Welcome, robots

We're building Litterbox, a tool for sandboxing agents doing software engineering. Our project has a few goals:

- Interoperate by use of MCP, the Model Control Protocol.
- Minimise external dependencies. We should need only a container runtime and an image to spawn in it.

# Stack

- Nix for dependency management
- Rust
  - `bollard` as Docker client
  - `clap` for CLI
  - `git2` for Git
  - `rmcp` for MCP

# Core concepts

- Sandboxes are pairs of SCM branches and some form of compute. SCM branches are the authoritative source of information about running sandboxes, and containers may be spun up from existing SCM branches in case of accidental deletion.

# Key Modules for Agents

When working on features related to project configuration or sandbox management, the following modules are particularly important:

- **`src/config_loader.rs`**: This module is responsible for loading, parsing, and merging project configuration files (`.litterbox.toml` and `.litterbox.local.toml`). Any changes to how project settings are defined or interpreted will likely involve this module.

- **`src/sandbox/mod.rs`**: This module defines the core `SandboxProvider` trait and its `DockerSandboxProvider` implementation. It orchestrates the creation, management, and tearing down of sandboxes, including Docker container interactions, source code provisioning, and command execution. Features involving sandbox lifecycle or container specifics will heavily utilize this module.

- **`src/config.rs`**: Defines the data structures for various configuration types. Modifications to project configuration schemas should start here.

- **`src/mcp.rs`**: Implements the Model Control Protocol server, exposing tools like `sandbox-create`. Changes to tool inputs, outputs, or new MCP tools will involve this module.
