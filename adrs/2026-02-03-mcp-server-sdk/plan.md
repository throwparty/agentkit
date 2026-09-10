# Technical Plan: Choosing an MCP Server SDK for Rust

## 1. Architecture Overview

This project is a Rust-based MCP server. The goal of this selection process is to choose the most suitable SDK to implement this server. The chosen SDK will handle the Model Context Protocol (MCP) transport and message parsing, allowing us to focus on tool implementation and core functionality.

The server will:

- Use the selected SDK for MCP communication.
- Be compiled as a standalone static binary (e.g., via `x86_64-unknown-linux-musl` for portability).
- Run locally on the user's machine, typically spawned by an MCP host.
- Load configuration from a TOML file.

## 2. Technology Stack Justification

- **Rust:** Mandated for performance, memory safety, and high-quality CLI tooling.
- **Static Binary:** Ensures a zero-dependency deployment model, critical for a local tool.
- **TOML:** Standard for Rust configuration (using `serde` and `toml` crates).
- **Async Runtime:** `tokio` is the industry standard and required by most MCP SDKs.
- **Serialization:** `serde` and `serde_json` for protocol handling.
- **Schema Generation:** `schemars` for generating JSON Schema definitions for tools.

## 3. Selection Process Components

This document defines the process for selecting the SDK. The key parts of this process are:

### 3.1 SDK Discovery

Identifying potential Rust MCP Server SDKs by searching crates.io, GitHub, and the official MCP documentation.

### 3.2 Evaluation Criteria Mapping

Mapping each candidate SDK against the requirements defined in `spec.md`.

### 3.3 Evaluation Matrix

A structured comparison of identified SDKs to objectively measure them against the defined non-functional requirements.

## 4. Decisions

### 4.1 MCP Server SDK Selection (2026-02-03)

**Status:** Pending (Evaluation in Progress)
**Candidates:** `rmcp`, `hyper-mcp`, `pmcp`, `ultrafast-mcp`, `mcp-protocol-sdk`, `rust-mcp-sdk`

#### Alternatives Considered

1. **`rmcp` (Official):**
   - **Repository:** [modelcontextprotocol/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)
   - **Pros:** Official implementation, strong macro support (`rmcp-macros`), active development.
   - **Cons:** Early stage (v0.14.0).
1. **`hyper-mcp`:**
   - **Repository:** [hyper-mcp-rs/hyper-mcp](https://github.com/hyper-mcp-rs/hyper-mcp)
   - **Pros:** High performance, WASM sandboxing support, multiple transports (SSE/HTTP).
   - **Cons:** Different architecture - you build WASM plugins loaded by the hyper-mcp runtime rather than a standalone binary.
   - **Note:** Uses Extism PDK for plugin development, compiled to wasm32-wasip1 target.
1. **`pmcp`:**
   - **Crate:** [pmcp](https://crates.io/crates/pmcp)
   - **Pros:** High TypeScript SDK compatibility, comprehensive feature set, very ergonomic.
1. **`ultrafast-mcp`:**
   - **Crate:** [ultrafast-mcp](https://crates.io/crates/ultrafast-mcp)
   - **Pros:** Focused on performance and ergonomics.
1. **`mcp-protocol-sdk` / `rust-mcp-sdk`:**
   - **Status:** Evaluated as community-driven alternatives. `mcp-protocol-sdk` offers production-ready traits and transport support.

#### Evaluation Matrix

| Criteria             | `rmcp`             | `hyper-mcp` | `pmcp`    | `ultrafast-mcp` |
| :------------------- | :----------------- | :---------- | :-------- | :-------------- |
| **Protocol Support** | Full (Official)    | Full        | Full      | Full            |
| **Ergonomics**       | Excellent (Macros) | Good        | Excellent | High            |
| **Security**         | Standard           | High (WASM) | Standard  | Standard        |
| **Community**        | Very High          | High        | High      | Medium          |
| **Reliability**      | High               | Medium      | High      | Medium          |

#### Justification

A final selection has not yet been made. We will evaluate all candidates by implementing the "write_file" proof-of-concept (PoC) for each to determine which best fits our requirements for ergonomics, performance, and stability.
`rmcp` currently stands as a strong candidate due to its official status, while `hyper-mcp` offers unique sandboxing benefits. The PoC phase will be the primary driver for the final decision.

## 5. Selection Process Workflow

The selection will follow these steps:

1. **Discovery:** Compile a list of candidate Rust crates that implement the MCP server role.
1. **Initial Filtering:** Discard candidates that are unmaintained or lack documentation.
1. **Detailed Analysis:** Evaluate API ergonomics and protocol completeness.
1. **Proof-of-Concept:** Implement the "write" tool PoC (defined below) using the top candidates.
1. **Final Recommendation:** Confirm the primary SDK choice based on the PoC experience.

## 6. Proof-of-Concept (PoC)

To validate the candidates, we will scaffold a basic MCP server that implements a single "write" tool.

### 6.1 PoC Functional Scope: The "Write" Tool

The PoC server will expose a tool named `write_file` with the following characteristics:

- **Arguments:**
  - `path`: A string representing the absolute path to the target file.
  - `content`: A string containing the full content to be written.
- **Behavior:** The server will receive the tool call, parse the arguments, and write the `content` to the disk at the specified `path`.
- **Validation:** This PoC will demonstrate how the SDK handles:
  - Tool registration and schema definition.
  - JSON-RPC message routing for tool calls.
  - Type-safe parameter extraction.
  - Synchronous or asynchronous execution of side effects (filesystem I/O).

## 7. Testing Strategy

### 7.1 Evaluation Testing

- **PoC Implementation:** Hands-on validation of the "write" tool as described above.
- **Crate Analysis:** Checking dependency trees and build times of the candidate SDKs.

### 7.2 Integration Testing (Post-Selection)

- **Protocol Compliance:** Using the MCP Inspector or similar tools to verify the server correctly implements the protocol.
- **I/O Validation:** Ensuring the "write" tool correctly handles various file paths and large content strings.

## 8. Deployment and Packaging

The final MCP server will be optimized for local use:

- **Static Linking:** The binary will be statically linked to ensure portability.
- **Local Execution:** The server will be designed to be spawned by an MCP host via standard I/O.
- **Configuration:** A `config.toml` file will be used to manage settings such as allowed write directories and logging levels.
