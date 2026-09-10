# hyper-mcp PoC Implementation Notes

## Overview

hyper-mcp v0.2.3 uses a **fundamentally different architecture** from traditional MCP SDKs. Instead of building a standalone MCP server binary, you build WASM plugins that are loaded by the hyper-mcp runtime.

## Architecture

- **Plugin Development**: Write Rust code targeting `wasm32-wasip1`
- **Runtime**: hyper-mcp runtime loads and executes WASM plugins
- **Sandboxing**: Plugins run in a WASM sandbox with restricted capabilities
- **Distribution**: Plugins distributed as WASM files (.wasm) via OCI registries or file:// URLs

## Dependencies Used

```toml
[dependencies]
anyhow = "1.0.100"
base64 = "0.22.1"
base64-serde = "0.8.0"
chrono = { version = "0.4.43", features = ["serde"] }
extism-pdk = "1.4.1"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
```

**Build target**: `wasm32-wasip1` (WebAssembly System Interface Preview 1)

## Key Learnings

### 1. PDK Type System

The Extism PDK types differ significantly from standard MCP types:

**Tool Definition**:

```rust
Tool {
    name: String,
    description: Option<String>,
    input_schema: ToolSchema,  // Not JSON Value!
    annotations: Option<Annotations>,
    output_schema: Option<ToolSchema>,
    title: Option<String>,
}
```

**ToolSchema**: Must use structured `ToolSchema` type, not `serde_json::Value`:

```rust
ToolSchema {
    r#type: ObjectType::Object,
    properties: Option<Map<String, Value>>,
    required: Option<Vec<String>>,
}
```

**CallToolRequest**: Arguments are nested:

```rust
input.request.name  // NOT input.name
input.request.arguments  // NOT input.arguments
```

**CallToolResult**: Content is an enum:

```rust
CallToolResult {
    content: Vec<ContentBlock>,  // NOT Vec<Content>
    ...
}

ContentBlock::Text(TextContent {
    text: String,
    r#type: TextType::Text,  // Required!
    meta: Option<Meta>,
    annotations: Option<Annotations>,
})
```

### 2. Build Process

**Required steps**:

1. Install wasm32-wasip1 target (via Nix in this project)
1. Build with: `cargo build --release --target wasm32-wasip1`
1. Output: `target/wasm32-wasip1/release/plugin.wasm` (391KB for this PoC)
1. Build time: ~4.65s

**Nix environment** (see `nix/flake.nix`):

```nix
rustToolchain = pkgs.rust-bin.stable.latest.default.override {
  targets = [ "wasm32-wasip1" ];
};
```

### 3. Testing Challenges

**Standard test harness doesn't work** because:

- Plugin is not a standalone binary
- Requires hyper-mcp runtime to load it
- Standard MCP stdio communication happens through hyper-mcp, not the plugin directly

**Custom test approach**:

1. Create `config.json` pointing to plugin:

   ```json
   {
     "plugins": {
       "write_file_plugin": {
         "url": "file:///path/to/plugin.wasm"
       }
     }
   }
   ```

1. Run hyper-mcp:

   ```bash
   hyper-mcp --config-file config.json --insecure-skip-signature true
   ```

1. Communicate via JSON-RPC over stdio with hyper-mcp (not the plugin)

### 4. Critical Limitation: WASM Sandbox

**The write_file tool cannot actually write files!**

Error encountered:

```
Failed to call plugin: failed to find a pre-opened file descriptor
through which "/tmp" could be opened
```

**Why?**:

- WASM plugins run in a security sandbox
- No direct filesystem access by design
- This is a **fundamental security feature** of hyper-mcp
- Plugins cannot perform arbitrary filesystem operations

**Implications**:

- hyper-mcp is excellent for compute/transform/analysis tools
- NOT suitable for tools requiring filesystem access, network I/O, etc.
- Host functions may provide limited capabilities (see template README)

## Test Results

✅ **Test 1: Initialize** - PASS

- Server: hyper-mcp v0.2.3
- Protocol version: 2024-11-05

✅ **Test 2: List Tools** - PASS

- Tool discovered: `write_file_plugin-write_file`
- Note: hyper-mcp prefixes tool names with plugin name

❌ **Test 3: Write Absolute Path** - FAIL (Expected)

- Error: No filesystem access from WASM sandbox
- This is by design, not a bug

⚠️ **Test 4: Reject Relative Path** - CANNOT TEST

- Would fail before validation due to sandbox restrictions

## Comparison Points

| Aspect           | hyper-mcp                  | rmcp (traditional SDK)     |
| ---------------- | -------------------------- | -------------------------- |
| Binary Type      | WASM plugin                | Standalone executable      |
| Deployment       | Requires hyper-mcp runtime | Self-contained             |
| File I/O         | Blocked (sandboxed)        | Full access                |
| Network          | Limited/blocked            | Full access                |
| Security         | High (WASM sandbox)        | Standard process isolation |
| Distribution     | OCI registries             | Direct binary              |
| Build Complexity | Requires WASM target       | Standard Rust build        |
| Runtime Deps     | hyper-mcp runtime required | None (standalone)          |

## Developer Experience

**Positives**:

- Clean PDK with good type definitions
- Template provides excellent starting point
- Build is fast (4.65s)
- Small binary size (391KB)
- Strong security guarantees

**Negatives**:

- Type system more complex than direct JSON
- Sandbox restrictions limit use cases
- Requires hyper-mcp runtime installation
- Cannot use standard MCP test tools
- Tool names get prefixed by hyper-mcp

## Recommended Use Cases

**Good fit**:

- Data transformation/analysis tools
- Computation-heavy operations
- LLM prompt generation
- Read-only operations (with host functions)
- Environments requiring strong sandboxing

**Poor fit**:

- File system manipulation (write/delete/move)
- Network operations
- System administration tools
- Database operations (unless via host functions)
- Anything requiring direct OS interaction

## Build Time

- Clean build: ~30s (including dependencies)
- Incremental build: ~4.65s
- Binary size: 391KB (WASM)

## Conclusion

hyper-mcp represents a **different paradigm** for MCP servers:

- **Security-first**: WASM sandboxing prevents malicious behavior
- **Composable**: Load multiple plugins into one runtime
- **Portable**: WASM runs anywhere
- **Limited**: Cannot perform privileged operations

For Litterbox's use case (filesystem manipulation in containers), hyper-mcp's sandbox restrictions are a **dealbreaker**. It's excellent for other use cases but not suitable for our requirements.

## Files Created

- `src/lib.rs` - Plugin implementation
- `src/pdk/` - PDK types and utilities (copied from template)
- `config.json` - hyper-mcp configuration
- `test_hyper_mcp.py` - Custom test script
- `Cargo.toml` - Project configuration with WASM crate-type
