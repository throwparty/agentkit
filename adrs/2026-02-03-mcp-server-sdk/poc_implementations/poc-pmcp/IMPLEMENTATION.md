# pmcp PoC Implementation

**Status**: ✅ **SUCCESS** - All tests passing with git main branch\
**Date**: 2026-02-04\
**SDK**: pmcp v1.9.4+git (commit e1bcebaf from main branch)

## Summary

pmcp **successfully passed all 4 tests** after using the main branch from git. However, this represents a **major stability concern**: the published v1.9.4 on crates.io has a broken stdio transport, and we must depend on unreleased code from the main branch.

**Critical Issue**: stdio support is non-functional in the latest stable release (v1.9.4). PR #157 fixed this on January 18, 2026, but no new release has been published yet (as of Feb 4, 2026 - 17 days later).

## Critical Discovery: Broken Release, Working Main

**Problem**: pmcp v1.9.4 published on crates.io has non-functional stdio transport

- Used HTTP-style `Content-Length:` framing instead of JSON-RPC newline protocol
- Would hang indefinitely when receiving MCP initialize requests

**Solution**: PR #157 fixed stdio transport in the main branch

- Repository: https://github.com/paiml/rust-mcp-sdk
- Fixed commit: e1bcebaf (merged Jan 18, 2026)
- Use git dependency until next release is published

## Test Results

**All 4 tests passed** when using main branch:

```
✓ Initialize successful: poc-pmcp-write-file v1.0.0
✓ Found write_file tool
✓ File created successfully with correct content
✓ Relative path correctly rejected: Validation error: path must be absolute
```

## Implementation

### Cargo.toml

```toml
[dependencies]
pmcp = { git = "https://github.com/paiml/rust-mcp-sdk", branch = "main" }
async-trait = "0.1.89"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
tokio = { version = "1.49", features = ["full"] }
```

### src/main.rs

```rust
use async_trait::async_trait;
use pmcp::server::auth::{NoOpAuthProvider, ScopeBasedAuthorizer};
use pmcp::{Server, ToolHandler, RequestHandlerExtra, ServerCapabilities};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct WriteFileResult {
    message: String,
    bytes_written: usize,
}

struct WriteFileTool;

#[async_trait]
impl ToolHandler for WriteFileTool {
    async fn handle(&self, args: Value, _extra: RequestHandlerExtra) -> pmcp::Result<Value> {
        let params: WriteFileArgs = serde_json::from_value(args)
            .map_err(|e| pmcp::Error::validation(format!("Invalid arguments: {}", e)))?;

        let path_buf = Path::new(&params.path);
        if !path_buf.is_absolute() {
            return Err(pmcp::Error::validation("path must be absolute"));
        }

        if let Some(parent) = path_buf.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                pmcp::Error::internal(format!("Failed to create parent directories: {}", e))
            })?;
        }

        fs::write(&params.path, &params.content).await.map_err(|e| {
            pmcp::Error::internal(format!("Failed to write file: {}", e))
        })?;

        Ok(serde_json::to_value(WriteFileResult {
            message: format!("Successfully wrote {} bytes to {}", params.content.len(), params.path),
            bytes_written: params.content.len(),
        })?)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let authorizer = ScopeBasedAuthorizer::new()
        .require_scopes("write_file", Vec::<String>::new())
        .default_scopes(vec!["mcp:tools:use".to_string()]);

    let server = Server::builder()
        .name("poc-pmcp-write-file")
        .version("1.0.0")
        .capabilities(ServerCapabilities::tools_only())
        .auth_provider(NoOpAuthProvider)
        .tool_authorizer(authorizer)
        .tool("write_file", WriteFileTool)
        .build()?;

    server.run_stdio().await?;
    Ok(())
}
```

## Key Implementation Notes

### 1. Auth Requirements (Even for stdio)

pmcp requires auth configuration even when not using authentication:

```rust
use pmcp::server::auth::{NoOpAuthProvider, ScopeBasedAuthorizer};

let authorizer = ScopeBasedAuthorizer::new()
    .require_scopes("write_file", Vec::<String>::new())
    .default_scopes(vec!["mcp:tools:use".to_string()]);

Server::builder()
    .auth_provider(NoOpAuthProvider)  // ← Required
    .tool_authorizer(authorizer)      // ← Required
```

This is more verbose than rmcp but provides a clear path for adding OAuth later.

### 2. Tool Handler Pattern

pmcp uses trait-based handlers instead of macros:

```rust
use async_trait::async_trait;

struct WriteFileTool;

#[async_trait]
impl ToolHandler for WriteFileTool {
    async fn handle(&self, args: Value, _extra: RequestHandlerExtra) -> pmcp::Result<Value> {
        // Manual deserialization
        let params: WriteFileArgs = serde_json::from_value(args)?;
        // Implementation
        Ok(serde_json::to_value(result)?)
    }
}
```

**Comparison to rmcp**:

- **pmcp**: Explicit trait implementation, manual JSON handling
- **rmcp**: `#[tool]` macro auto-generates everything from function signature

### 3. Error Handling

pmcp provides domain-specific error constructors:

```rust
pmcp::Error::validation("path must be absolute")  // -32602 Invalid params
pmcp::Error::internal("database connection failed") // -32603 Internal error
```

## Build Environment Fix

The nix flake originally had `CC_FOR_BUILD=null` which broke linking for crates with C dependencies (like pmcp's `ring` crate). Fixed in `nix/flake.nix`:

```nix
pkgs.mkShell {
  nativeBuildInputs = [  # Changed from buildInputs
    cargo-auditable
    python3
    rustToolchain
    stdenv.cc  # ← Added: provides proper C compiler
  ];
}
```

**Critical**: All cargo builds must run inside `nix develop ./nix`:

```bash
nix develop ./nix --command bash -c 'cd poc_implementations && python3 test_mcp_server.py poc-pmcp'
```

## pmcp Architecture

pmcp is designed for **HTTP-first development with cloud deployment workflows**:

### Primary Workflow (HTTP)

```bash
cargo pmcp new my-workspace     # Create workspace
cargo pmcp add myserver --tools # Add server
cargo pmcp dev --server myserver # Start HTTP server on :3000
cargo pmcp test --server myserver # Test via HTTP
cargo pmcp deploy               # Deploy to AWS/GCP/Cloudflare
```

### stdio Support (Secondary)

- `run_stdio()` method exists for local development
- **Now working** in main branch (fixed in PR #157)
- Less documented than HTTP workflow
- No `cargo pmcp` tooling for stdio (no hot-reload, etc.)

## Comparison: pmcp vs rmcp

| Feature               | rmcp                           | pmcp                          |
| --------------------- | ------------------------------ | ----------------------------- |
| **STDIO support**     | ✅ Primary, fully working      | ✅ Working (main branch only) |
| **HTTP support**      | ❌ None                        | ✅ Primary with SSE           |
| **Macros**            | ✅ `#[tool]`, `#[tool_router]` | ❌ Trait-based handlers       |
| **Cloud deployment**  | ❌ Manual                      | ✅ `cargo pmcp deploy`        |
| **OAuth**             | ❌ None                        | ✅ Cognito, OIDC, DCR         |
| **Hot reload**        | ❌ None                        | ✅ `cargo pmcp dev`           |
| **Test generation**   | ❌ Manual                      | ✅ `--generate-scenarios`     |
| **Boilerplate**       | Low (macros)                   | Medium (traits + auth)        |
| **Use case**          | Local tools, simple servers    | Production cloud services     |
| **Crates.io release** | ✅ Latest works                | ❌ Broken, use git main       |

## pmcp Strengths

✅ **Production-ready cloud deployments** - One command to AWS Lambda/GCP/Cloudflare\
✅ **Comprehensive OAuth** - Cognito integration, tenant ID extraction, DCR\
✅ **Developer tooling** - `cargo-pmcp` CLI with hot-reload and test generation\
✅ **Infrastructure as code** - AWS CDK stacks included\
✅ **Multi-tenancy** - Built-in tenant isolation\
✅ **Extensive examples** - 60+ examples covering all features

## pmcp Weaknesses

⚠️ **BLOCKING ISSUE: Depends on unreleased code**

- **Latest stable release (v1.9.4) has broken stdio** - unusable for our requirements
- **Must use git main branch** - requires `git = "..."` dependency instead of semantic versioning
- **No new release published** - Fix merged Jan 18, 2026; still unreleased 17 days later
- **Risk of breaking changes** - main branch has no semver guarantees
- **Dependency instability** - Pinned to commit e1bcebaf, could diverge from eventual release
- **Production concerns** - Using unreleased code in production is risky

⚠️ **Complex for simple servers** - Auth boilerplate required even for stdio\
⚠️ **Less documentation for stdio** - Most docs focus on HTTP workflow\
⚠️ **Heavier dependencies** - Pulls in HTTP stack even for stdio-only use

## Recommendation

### ⚠️ pmcp Cannot Be Recommended for Production Use

**Reason**: The latest stable release has broken stdio support. Using unreleased git dependencies creates:

- **No semantic versioning** - Can't specify version constraints like `^1.9`
- **Build reproducibility issues** - Commit hashes are fragile, branches can change
- **Dependency audit failures** - Security scanners flag git dependencies
- **Maintenance burden** - Must manually track when releases are published

### Use pmcp ONLY if:

- You accept the risk of depending on unreleased code from git main
- Deploying to AWS Lambda, Google Cloud Run, or Cloudflare Workers (and the cloud features justify the risk)
- Need OAuth authentication (Cognito, OIDC)
- Building multi-tenant SaaS services

**Better alternative**: Wait for pmcp to publish a new stable release before adopting

### Use rmcp for production (stable, released):

- Building local CLI tools or simple integrations
- Want minimal boilerplate (macros over traits)
- Prefer stdio as primary transport
- Need **stable, versioned dependencies from crates.io**

### Git dependency workaround (until new release):

```toml
# ⚠️ NOT RECOMMENDED FOR PRODUCTION
pmcp = { git = "https://github.com/paiml/rust-mcp-sdk", branch = "main" }
# Or pin to specific commit:
pmcp = { git = "https://github.com/paiml/rust-mcp-sdk", rev = "e1bcebaf" }
```

## Files

- `src/main.rs` - write_file tool implementation (94 lines)
- `Cargo.toml` - Git dependency on pmcp main branch
- Build: 2.4MB binary when compiled in nix shell
