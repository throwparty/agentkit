//! The client-capability tool descriptors: the ACP client's file-system
//! and terminal surface, reflected to the model as `client.*` tools.
//!
//! Descriptors only. The RPC fixes the parameters, so no schema is
//! authored beyond the wire types themselves. A tool exists only when the
//! client advertised the capability at `initialize` and the category is
//! enabled (`[client_tools]`, overridable at runtime with `/editor`).
//! Executing a call against the live connection is the ACP glue layer's
//! job; the catalogue here is the domain knowledge of what exists.

/// A tool offered to the model, derived from the client's surface.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientTool {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

pub const FS_READ: &str = "client.fs.read_text_file";
pub const FS_WRITE: &str = "client.fs.write_text_file";
pub const TERMINAL_CREATE: &str = "client.terminal.create";
pub const TERMINAL_OUTPUT: &str = "client.terminal.output";
pub const TERMINAL_WAIT: &str = "client.terminal.wait_for_exit";
pub const TERMINAL_KILL: &str = "client.terminal.kill";
pub const TERMINAL_RELEASE: &str = "client.terminal.release";

/// Which client-tool categories are enabled for this connection: the
/// client's advertised capability AND the category toggle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientToolSelection {
    pub fs_read: bool,
    pub fs_write: bool,
    pub terminal: bool,
}

fn object(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn terminal_id_schema() -> serde_json::Value {
    object(
        serde_json::json!({
            "terminal_id": { "type": "string", "description": "A terminal id returned by client.terminal.create." }
        }),
        &["terminal_id"],
    )
}

/// The tools whose capability is advertised and whose category is on.
pub fn catalog(selection: ClientToolSelection) -> Vec<ClientTool> {
    let mut tools = Vec::new();

    if selection.fs_read {
        tools.push(ClientTool {
            name: FS_READ.to_owned(),
            description: "Read a file the editor has opened read access to, in the user's workspace."
                .to_owned(),
            schema: object(
                serde_json::json!({
                    "path": { "type": "string", "description": "Absolute path of the file to read." },
                    "line": { "type": "integer", "description": "One-based line to start from." },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read." }
                }),
                &["path"],
            ),
        });
    }

    if selection.fs_write {
        tools.push(ClientTool {
            name: FS_WRITE.to_owned(),
            description: "Write a file in the user's workspace through the editor.".to_owned(),
            schema: object(
                serde_json::json!({
                    "path": { "type": "string", "description": "Absolute path of the file to write." },
                    "content": { "type": "string", "description": "The full file content." }
                }),
                &["path", "content"],
            ),
        });
    }

    if selection.terminal {
        tools.push(ClientTool {
            name: TERMINAL_CREATE.to_owned(),
            description: "Run a command in a new terminal in the editor's environment.".to_owned(),
            schema: object(
                serde_json::json!({
                    "command": { "type": "string", "description": "The program to run." },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "env": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Extra environment variables."
                    },
                    "cwd": { "type": "string", "description": "Working directory." },
                    "output_byte_limit": { "type": "integer", "description": "Maximum output bytes to retain." }
                }),
                &["command"],
            ),
        });
        tools.push(ClientTool {
            name: TERMINAL_OUTPUT.to_owned(),
            description: "The current output of a terminal created with client.terminal.create."
                .to_owned(),
            schema: terminal_id_schema(),
        });
        tools.push(ClientTool {
            name: TERMINAL_WAIT.to_owned(),
            description: "Wait for a terminal to exit and return its exit status.".to_owned(),
            schema: terminal_id_schema(),
        });
        tools.push(ClientTool {
            name: TERMINAL_KILL.to_owned(),
            description: "Kill a running terminal.".to_owned(),
            schema: terminal_id_schema(),
        });
        tools.push(ClientTool {
            name: TERMINAL_RELEASE.to_owned(),
            description: "Release a finished terminal, freeing the editor's resources.".to_owned(),
            schema: terminal_id_schema(),
        });
    }

    tools
}
