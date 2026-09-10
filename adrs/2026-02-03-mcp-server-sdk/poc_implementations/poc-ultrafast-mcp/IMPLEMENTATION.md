# ultrafast-mcp PoC Implementation

**Status**: ✅ **PASS** - All tests passing with patched version

**Date**: 2026-02-04\
**SDK**: ultrafast-mcp v202506018.1.0 (patched)

## Summary

ultrafast-mcp v202506018.1.0 **has a feature flag bug** in the published crate that prevents stdio-only usage. With a simple 4-line patch to fix the feature flag issue, **all 4 tests pass successfully**.

**Upstream Fix**: PR #6 submitted to techgopal/ultrafast-mcp - https://github.com/techgopal/ultrafast-mcp/pull/6 (awaiting merge)

## SDK Information

- **Crate**: `ultrafast-mcp` v202506018.1.0
- **License**: MIT OR Apache-2.0
- **Documentation**: https://docs.rs/ultrafast-mcp
- **Repository**: https://github.com/techgopal/ultrafast-mcp
- **Features Used**: `stdio`, `core`

## Implementation Details

### Dependencies

```toml
ultrafast-mcp = { version = "202506018.1.0", features = ["stdio", "core"] }
tokio = { version = "1.49.0", features = ["full"] }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
anyhow = "1.0.100"
schemars = "1.2.1"
```

### Code Structure

The implementation follows ultrafast-mcp's documented pattern:

1. **Tool Handler**: Implement `ToolHandler` trait with two methods:
   - `handle_tool_call(&self, call: ToolCall) -> MCPResult<ToolResult>` - Execute tool logic
   - `list_tools(&self, request: ListToolsRequest) -> MCPResult<ListToolsResponse>` - List available tools

1. **Server Setup**: Create `UltraFastServer` with:
   - `ServerInfo` (name, version, description)
   - `ServerCapabilities` (advertise tools capability)
   - Tool handler registered via `.with_tool_handler(Arc::new(handler))`

1. **Transport**: Use `.run_stdio().await` to start STDIO transport

### API Ergonomics

ultrafast-mcp provides:

- **Comprehensive prelude**: `use ultrafast_mcp::prelude::*` imports all common types
- **Type-safe errors**: `MCPError` enum with helpers like `invalid_params()`, `internal_error()`
- **Builder patterns**: `ServerCapabilities::builder()`, tool configuration
- **Rich documentation**: Extensive inline docs with examples
- **Modern async**: Full async/await support with tokio

### Comparison with rmcp

| Feature       | ultrafast-mcp              | rmcp                               |
| ------------- | -------------------------- | ---------------------------------- |
| Macros        | No macros (trait-based)    | `#[tool_router]`, `#[tool]` macros |
| Boilerplate   | Medium (manual trait impl) | Low (macros generate code)         |
| Type safety   | Excellent                  | Excellent                          |
| Documentation | Very comprehensive         | Good                               |
| Complexity    | Higher (more explicit)     | Lower (macro magic)                |

## Test Results

**All 4 tests passed** with patched version:

```
✓ Initialize successful: poc-ultrafast-mcp v1.0.0
✓ Found write_file tool: write_file
  Description: Write content to a file at the specified path
✓ File created successfully with correct content
✓ Relative path correctly rejected: Tool call failed: Protocol error: Serialization error: path must be absolute
```

## The Bug

**Location**: `crates/ultrafast-mcp/src/lib.rs` lines 487-490

**Problem**: Middleware imports were inside `#[cfg(feature = "stdio")]` block:

```rust
#[cfg(feature = "stdio")]
pub use ultrafast_mcp_transport::{
    Transport,
    TransportConfig,
    create_recovering_transport,
    create_transport,
    // ❌ BUG: These imports require the http feature!
    streamable_http::middleware::{
        LoggingMiddleware, MiddlewareTransport, ProgressMiddleware,
        RateLimitMiddleware, TransportMiddleware, ValidationMiddleware,
    },
    stdio::StdioTransport,
};
```

But `streamable_http` module only exists when `feature = "http"`:

```rust
#[cfg(feature = "http")]
pub mod streamable_http;
```

**Result**: Compilation fails when using `minimal` or `stdio` features without `http`.

## The Fix

**Patch applied to local clone** at `~/Code/techgopal/ultrafast-mcp`:

1. **Removed** middleware imports from stdio block (lines 487-490)
1. **Moved** middleware imports to http block where they belong

```rust
// BEFORE (broken)
#[cfg(feature = "stdio")]
pub use ultrafast_mcp_transport::{
    streamable_http::middleware::{...},  // ❌
    stdio::StdioTransport,
};

// AFTER (fixed)
#[cfg(feature = "stdio")]
pub use ultrafast_mcp_transport::{
    stdio::StdioTransport,
};

#[cfg(feature = "http")]
pub use ultrafast_mcp_transport::streamable_http::{
    ...,
    middleware::{...},  // ✅ Now in correct feature block
};
```

## Implementation Details

### Dependencies

```toml
ultrafast-mcp = { path = "/Users/lukecarrier/Code/techgopal/ultrafast-mcp/crates/ultrafast-mcp", features = ["minimal"] }
tokio = { version = "1.49.0", features = ["full"] }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
anyhow = "1.0.100"
async-trait = "0.1.89"
```

### API Pattern

ultrafast-mcp uses trait-based handlers:

```rust
use ultrafast_mcp::{
    ListToolsRequest, ListToolsResponse, MCPError, MCPResult,
    ServerCapabilities, ServerInfo, Tool, ToolCall, ToolContent,
    ToolHandler, ToolResult, ToolsCapability, UltraFastServer,
};

#[async_trait::async_trait]
impl ToolHandler for WriteFileHandler {
    async fn handle_tool_call(&self, call: ToolCall) -> MCPResult<ToolResult> {
        // Handle tool execution
        Ok(ToolResult {
            content: vec![ToolContent::text(response_json)],
            is_error: Some(false),
        })
    }

    async fn list_tools(&self, _request: ListToolsRequest) -> MCPResult<ListToolsResponse> {
        // Return tool schemas
    }
}
```

**Key differences from rmcp**:

- **No macros** - Manual trait implementation
- **More verbose** - Explicit error types and constructors
- **Type-safe errors** - `MCPError::serialization_error()`, etc.
- **Structured types** - All protocol types explicitly defined

## Recommendation

**Current Status**: ⚠️ **CANNOT RECOMMEND** until PR #6 is merged and released

### Why Not Recommended (Yet)

1. **Published version is broken** - v202506018.1.0 on crates.io doesn't compile for stdio-only use
1. **Requires local patch** - Must clone and patch locally, or wait for PR merge
1. **No release timeline** - Unknown when fix will be published to crates.io
1. **Same issue as pmcp** - Requires using unreleased code (git dependency or local path)

### After PR #6 is Merged and Released

ultrafast-mcp would be **worth reconsidering** if:

- The fix is merged and a new version published to crates.io
- stdio-only features work without the http feature
- You value its strengths over rmcp's simplicity

### ultrafast-mcp Strengths (vs rmcp)

✅ **Comprehensive documentation** - Extensive API docs and examples\
✅ **Production features** - Monitoring, auth, multiple transports\
✅ **Feature modularity** - Fine-grained feature flags\
✅ **Type-safe errors** - Domain-specific error constructors\
✅ **Latest MCP spec** - MCP 2025-06-18 compliant

### ultrafast-mcp Weaknesses (vs rmcp)

⚠️ **More verbose** - Trait implementations vs macros\
⚠️ **Larger dependency tree** - More crates to compile\
⚠️ **No macros** - More boilerplate code required\
⚠️ **Broken release** - Currently requires patch for stdio use

## Comparison: ultrafast-mcp vs rmcp

| Feature                 | rmcp                           | ultrafast-mcp (patched)     |
| ----------------------- | ------------------------------ | --------------------------- |
| **STDIO support**       | ✅ Works out of box            | ✅ Works with patch         |
| **Stable release**      | ✅ v0.14.0                     | ❌ v202506018.1.0 broken    |
| **Macros**              | ✅ `#[tool]`, `#[tool_router]` | ❌ Manual traits            |
| **Boilerplate**         | Low                            | Medium-High                 |
| **Error handling**      | Good                           | Excellent (typed)           |
| **Documentation**       | Good                           | Excellent                   |
| **Production features** | Basic                          | Advanced (monitoring, auth) |
| **Feature flags**       | Basic                          | Modular (stdio/http/oauth)  |
| **Use case**            | Simple servers                 | Production services         |

## Files

- `src/main.rs` - write_file tool implementation
- `Cargo.toml` - Local path dependency to patched ultrafast-mcp
- `~/Code/techgopal/ultrafast-mcp/` - Cloned repo with patch applied
- Patch: 4 lines removed from stdio block, moved to http block
