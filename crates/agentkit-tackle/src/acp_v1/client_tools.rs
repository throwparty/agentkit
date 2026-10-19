//! Client-capability tools: the ACP client's advertised file-system and
//! terminal surface, reflected to the model as `client.*` tools.
//!
//! Nothing is authored: the RPC fixes the parameters, so the harness
//! ships no schemas beyond the wire types themselves. A tool exists only
//! when the client advertised the capability at `initialize` and the
//! category is enabled (`[client_tools]`, overridable at runtime with
//! `/editor`).
//!
//! Terminals are connection-scoped and ephemeral: the harness tracks the
//! ids it created for a session and releases them at session close; a
//! stale id (reaped by the editor, or lost across a restart) fails the
//! call precisely rather than silently.

use crate::config::Config;
use crate::invokables::client_tools::{
    ClientTool, ClientToolExecutor, ClientToolSelection, Executed, FS_READ, FS_WRITE,
    TERMINAL_CREATE, TERMINAL_KILL, TERMINAL_OUTPUT, TERMINAL_RELEASE, TERMINAL_WAIT,
    catalog as selection_catalog,
};
use agent_client_protocol::schema::v1;
use agent_client_protocol::{Client, ConnectionTo};

/// The client capabilities captured at `initialize`, the source of truth
/// for which `client.*` tools exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientCaps {
    pub fs_read: bool,
    pub fs_write: bool,
    pub terminal: bool,
}

impl ClientCaps {
    pub fn from_capabilities(capabilities: &v1::ClientCapabilities) -> Self {
        Self {
            fs_read: capabilities.fs.read_text_file,
            fs_write: capabilities.fs.write_text_file,
            terminal: capabilities.terminal,
        }
    }
}

/// The runtime `/editor` toggles, seeded from `[client_tools]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorToggles {
    pub fs: bool,
    pub terminal: bool,
}

impl EditorToggles {
    /// The seed from configuration: absent `[client_tools]` leaves both
    /// categories on (subject to the client's advertised capabilities).
    pub fn from_config(config: &Config) -> Self {
        let section = config.client_tools.clone().unwrap_or_default();
        Self {
            fs: section.fs_enabled,
            terminal: section.terminal_enabled,
        }
    }
}

/// The client-tool catalogue for a connection: the advertised capability
/// AND the category toggle, resolved into the invokable descriptors.
pub fn catalog(caps: ClientCaps, toggles: EditorToggles) -> Vec<ClientTool> {
    selection_catalog(ClientToolSelection {
        fs_read: caps.fs_read && toggles.fs,
        fs_write: caps.fs_write && toggles.fs,
        terminal: caps.terminal && toggles.terminal,
    })
}

/// Executes client-capability tools against a live connection.
pub struct ConnectionClientTools {
    pub cx: ConnectionTo<Client>,
}

impl ClientToolExecutor for ConnectionClientTools {
    fn execute(
        &self,
        session: &crate::store::SessionId,
        invokable: &str,
        arguments: &serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Executed, String>> + Send + '_>>
    {
        let wire = crate::state::wire_session_id(session);
        let cx = self.cx.clone();
        let invokable = invokable.to_owned();
        let arguments = arguments.clone();
        Box::pin(async move { execute(&cx, &wire, &invokable, &arguments).await })
    }
}

fn string_argument(arguments: &serde_json::Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

/// Executes one `client.*` call against the live connection. A stale
/// terminal id, or any RPC failure, is a precise error string — never a
/// panic and never a silent success.
pub async fn execute(
    cx: &ConnectionTo<Client>,
    session_id: &v1::SessionId,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<Executed, String> {
    let plain = |content: String| Executed {
        content,
        created: None,
        finished: None,
    };
    match name {
        FS_READ => {
            let path = string_argument(arguments, "path")?;
            let line = arguments.get("line").and_then(serde_json::Value::as_u64);
            let limit = arguments.get("limit").and_then(serde_json::Value::as_u64);
            let request = v1::ReadTextFileRequest::new(session_id.clone(), path)
                .line(line.map(|value| value as u32))
                .limit(limit.map(|value| value as u32));
            let response = cx
                .send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            Ok(plain(response.content))
        }
        FS_WRITE => {
            let path = string_argument(arguments, "path")?;
            let content = string_argument(arguments, "content")?;
            let bytes = content.len();
            let request = v1::WriteTextFileRequest::new(session_id.clone(), path.clone(), content);
            cx.send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            Ok(plain(format!("Wrote {bytes} bytes to {path}")))
        }
        TERMINAL_CREATE => {
            let command = string_argument(arguments, "command")?;
            let args: Vec<String> = arguments
                .get("args")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let env: Vec<v1::EnvVariable> = arguments
                .get("env")
                .and_then(|value| value.as_object())
                .map(|pairs| {
                    pairs
                        .iter()
                        .map(|(name, value)| {
                            v1::EnvVariable::new(name.clone(), value.as_str().unwrap_or_default())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let cwd = arguments
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(std::path::PathBuf::from);
            let limit = arguments
                .get("output_byte_limit")
                .and_then(serde_json::Value::as_u64);
            let request = v1::CreateTerminalRequest::new(session_id.clone(), command)
                .args(args)
                .env(env)
                .cwd(cwd)
                .output_byte_limit(limit);
            let response = cx
                .send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            let terminal_id = response.terminal_id.0.to_string();
            Ok(Executed {
                content: format!("Terminal {terminal_id} created"),
                created: Some(terminal_id),
                finished: None,
            })
        }
        TERMINAL_OUTPUT => {
            let terminal_id = terminal_id_argument(arguments)?;
            let request = v1::TerminalOutputRequest::new(session_id.clone(), terminal_id.clone());
            let response = cx
                .send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            let mut content = response.output;
            if response.truncated {
                content.push_str("\n[output truncated]");
            }
            if let Some(exit) = response.exit_status {
                content.push_str(&format!("\n[{}]", exit_line(&exit)));
            }
            Ok(plain(content))
        }
        TERMINAL_WAIT => {
            let terminal_id = terminal_id_argument(arguments)?;
            let request =
                v1::WaitForTerminalExitRequest::new(session_id.clone(), terminal_id.clone());
            let response = cx
                .send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            Ok(plain(format!(
                "Terminal {terminal_id} exited ({})",
                exit_line(&response.exit_status)
            )))
        }
        TERMINAL_KILL => {
            let terminal_id = terminal_id_argument(arguments)?;
            let text = terminal_id.0.to_string();
            let request = v1::KillTerminalRequest::new(session_id.clone(), terminal_id.clone());
            cx.send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            // Q8: a killed terminal is released too, best-effort — the
            // command is dead, so the editor's terminal holds no work.
            let release = v1::ReleaseTerminalRequest::new(session_id.clone(), terminal_id);
            let _ = cx.send_request(release).block_task().await;
            Ok(Executed {
                content: format!("Terminal {text} killed"),
                created: None,
                finished: Some(text),
            })
        }
        TERMINAL_RELEASE => {
            let terminal_id = terminal_id_argument(arguments)?;
            let text = terminal_id.0.to_string();
            let request = v1::ReleaseTerminalRequest::new(session_id.clone(), terminal_id.clone());
            cx.send_request(request)
                .block_task()
                .await
                .map_err(|err| err.to_string())?;
            Ok(Executed {
                content: format!("Terminal {text} released"),
                created: None,
                finished: Some(text),
            })
        }
        other => Err(format!("unknown client tool `{other}`")),
    }
}

fn terminal_id_argument(arguments: &serde_json::Value) -> Result<v1::TerminalId, String> {
    string_argument(arguments, "terminal_id").map(v1::TerminalId::new)
}

fn exit_line(status: &v1::TerminalExitStatus) -> String {
    match (status.exit_code, status.signal.as_deref()) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "exit status unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_gates_on_capabilities_and_toggles() {
        let none = ClientCaps::default();
        let all = ClientCaps {
            fs_read: true,
            fs_write: true,
            terminal: true,
        };
        let on = EditorToggles {
            fs: true,
            terminal: true,
        };

        assert!(catalog(none, on).is_empty());

        let names: Vec<String> = catalog(all, on).into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                FS_READ,
                FS_WRITE,
                TERMINAL_CREATE,
                TERMINAL_OUTPUT,
                TERMINAL_WAIT,
                TERMINAL_KILL,
                TERMINAL_RELEASE,
            ]
        );

        // A category toggled off drops its tools.
        let fs_only = catalog(
            all,
            EditorToggles {
                fs: true,
                terminal: false,
            },
        );
        let names: Vec<&str> = fs_only.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec![FS_READ, FS_WRITE]);

        // A read-only client exposes only the read tool.
        let read_only = ClientCaps {
            fs_read: true,
            ..ClientCaps::default()
        };
        let read_only_tools = catalog(read_only, on);
        let names: Vec<&str> = read_only_tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec![FS_READ]);
    }

    #[test]
    fn config_seeds_the_toggles() {
        let defaults = EditorToggles::from_config(&Config::default());
        assert_eq!(
            defaults,
            EditorToggles {
                fs: true,
                terminal: true
            }
        );

        let toml = "[client_tools]\nfs_enabled = false\nterminal_enabled = false\n";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            EditorToggles::from_config(&config),
            EditorToggles {
                fs: false,
                terminal: false
            }
        );
    }
}
