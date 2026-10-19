//! Human-readable, argument-aware titles for tool calls.
//!
//! ACP renders a tool call by its `title`; the namespaced invokable name
//! (`client.fs.read_text_file`) is a poor label. These helpers derive a
//! short, informative title from the invokable and its arguments — "Read
//! /etc/hosts" rather than "client.fs.read_text_file" — and a `ToolKind`
//! hint for the client's iconography. The invokable name still rides the
//! `name` field, so nothing is lost.

use serde_json::Value;

/// The maximum length of a value embedded in a title.
const VALUE_LIMIT: usize = 100;

/// The iconography hint for an invokable, independent of the wire type the
/// ACP layer maps it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Execute,
    Other,
}

/// A short, argument-aware title for a tool call.
pub fn tool_call_title(name: &str, arguments: &Value) -> String {
    if let Some(rest) = name.strip_prefix("client.") {
        return client_title(rest, arguments);
    }
    if let Some(rest) = name.strip_prefix("mcp.") {
        return labelled(rest, arguments);
    }
    if let Some(rest) = name.strip_prefix("prompt.") {
        return labelled(&format!("prompt {rest}"), arguments);
    }
    if let Some(rest) = name.strip_prefix("agent.") {
        return labelled(&format!("agent {rest}"), arguments);
    }
    name.to_owned()
}

/// The client's icon hint for an invokable.
pub fn tool_kind(name: &str) -> ToolKind {
    match name {
        "client.fs.read_text_file" => ToolKind::Read,
        "client.fs.write_text_file" => ToolKind::Edit,
        _ if name.starts_with("client.terminal.") => ToolKind::Execute,
        _ => ToolKind::Other,
    }
}

/// `label: primary-argument`, or just `label` when no argument stands out.
fn labelled(label: &str, arguments: &Value) -> String {
    match primary_argument(arguments) {
        Some(value) => format!("{label}: {value}"),
        None => label.to_owned(),
    }
}

fn client_title(rest: &str, arguments: &Value) -> String {
    match rest {
        "fs.read_text_file" => {
            let path = string_argument(arguments, "path").unwrap_or_default();
            match (
                arguments.get("line").and_then(Value::as_u64),
                arguments.get("limit").and_then(Value::as_u64),
            ) {
                (Some(line), Some(limit)) => format!(
                    "Read {path} (lines {line}–{})",
                    line.saturating_add(limit.saturating_sub(1))
                ),
                (Some(line), None) => format!("Read {path} (from line {line})"),
                _ => format!("Read {path}"),
            }
        }
        "fs.write_text_file" => format!(
            "Write {}",
            string_argument(arguments, "path").unwrap_or_default()
        ),
        "terminal.create" => {
            let command = string_argument(arguments, "command").unwrap_or_default();
            let args: Vec<&str> = arguments
                .get("args")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let line = if args.is_empty() {
                command
            } else {
                format!("{command} {}", args.join(" "))
            };
            format!("Run {}", truncate(&line))
        }
        "terminal.output" => terminal_title("Read output of terminal", arguments),
        "terminal.wait_for_exit" => terminal_title("Wait for terminal", arguments),
        "terminal.kill" => terminal_title("Kill terminal", arguments),
        "terminal.release" => terminal_title("Release terminal", arguments),
        other => format!("client.{other}"),
    }
}

fn terminal_title(prefix: &str, arguments: &Value) -> String {
    match string_argument(arguments, "terminal_id") {
        Some(id) => format!("{prefix} {id}"),
        None => prefix.to_owned(),
    }
}

fn string_argument(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The most descriptive scalar argument: a known key wins, else the first
/// string value. Embedded values are truncated.
fn primary_argument(arguments: &Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "path",
        "file",
        "file_path",
        "command",
        "query",
        "pattern",
        "url",
        "prompt",
        "text",
        "name",
        "id",
        "terminal_id",
    ];
    let object = arguments.as_object()?;
    for key in KEYS {
        if let Some(value) = object.get(*key).and_then(Value::as_str) {
            return Some(truncate(value));
        }
    }
    object.values().find_map(Value::as_str).map(truncate)
}

/// Trims and truncates a value for embedding, never splitting a
/// multi-byte character.
fn truncate(value: &str) -> String {
    let value = value.trim();
    let mut end = value.len().min(VALUE_LIMIT);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if end < value.len() {
        format!("{}…", &value[..end])
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_reads_show_the_path() {
        assert_eq!(
            tool_call_title("client.fs.read_text_file", &json!({ "path": "/etc/hosts" })),
            "Read /etc/hosts"
        );
        assert_eq!(
            tool_call_title(
                "client.fs.read_text_file",
                &json!({ "path": "/a.rs", "line": 10, "limit": 5 })
            ),
            "Read /a.rs (lines 10–14)"
        );
        assert_eq!(
            tool_call_title(
                "client.fs.read_text_file",
                &json!({ "path": "/a.rs", "line": 10 })
            ),
            "Read /a.rs (from line 10)"
        );
    }

    #[test]
    fn writes_and_terminals_read_naturally() {
        assert_eq!(
            tool_call_title("client.fs.write_text_file", &json!({ "path": "/a.rs" })),
            "Write /a.rs"
        );
        assert_eq!(
            tool_call_title(
                "client.terminal.create",
                &json!({ "command": "cargo", "args": ["test"] })
            ),
            "Run cargo test"
        );
        assert_eq!(
            tool_call_title(
                "client.terminal.wait_for_exit",
                &json!({ "terminal_id": "t1" })
            ),
            "Wait for terminal t1"
        );
        assert_eq!(
            tool_call_title("client.terminal.output", &json!({ "terminal_id": "t1" })),
            "Read output of terminal t1"
        );
    }

    #[test]
    fn mcp_tools_show_a_primary_argument() {
        assert_eq!(
            tool_call_title(
                "mcp.github.search_issues",
                &json!({ "query": "is:open label:bug" })
            ),
            "github.search_issues: is:open label:bug"
        );
        // No scalar argument: the namespaced name alone.
        assert_eq!(tool_call_title("mcp.echo.echo", &json!({})), "echo.echo");
    }

    #[test]
    fn long_values_are_truncated_on_a_char_boundary() {
        let long = "x".repeat(VALUE_LIMIT + 50);
        let title = tool_call_title("mcp.s.t", &json!({ "query": long }));
        assert!(title.ends_with('…'), "{title}");
        assert!(title.chars().count() <= "s.t: ".len() + VALUE_LIMIT + 1);
    }

    #[test]
    fn unknown_names_pass_through() {
        assert_eq!(
            tool_call_title("something.else", &json!({})),
            "something.else"
        );
    }

    #[test]
    fn kinds_hint_the_iconography() {
        assert_eq!(tool_kind("client.fs.read_text_file"), ToolKind::Read);
        assert_eq!(tool_kind("client.fs.write_text_file"), ToolKind::Edit);
        assert_eq!(tool_kind("client.terminal.create"), ToolKind::Execute);
        assert_eq!(tool_kind("mcp.echo.echo"), ToolKind::Other);
    }
}
