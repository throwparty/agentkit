use clap::Arg;

// ---------------------------------------------------------------------------
// CLI docgen
// ---------------------------------------------------------------------------

/// Generate CLI reference documentation from a clap Command.
///
/// Returns a Markdown-formatted string with:
/// - Subcommands listed alphabetically
/// - Each subcommand's description (long_about or about)
/// - Positional arguments and options formatted consistently
pub fn generate_cli_docs(command: &clap::Command) -> String {
    command.clone().build();

    let mut output = String::new();
    output.push_str("# 👩‍💻 Command line interface\n\n");

    let mut subcommands: Vec<_> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "docgen")
        .collect();
    subcommands.sort_by(|a, b| a.get_name().cmp(b.get_name()));

    for subcommand in subcommands {
        output.push_str(&format!("## `{}`\n\n", subcommand.get_name()));

        if let Some(about) = subcommand
            .get_long_about()
            .or_else(|| subcommand.get_about())
        {
            output.push_str(&about.to_string());
            output.push_str("\n\n");
        }

        let (positionals, options): (Vec<&Arg>, Vec<&Arg>) = subcommand
            .get_arguments()
            .filter(|arg| !arg.is_hide_set())
            .partition(|arg| arg.is_positional());

        if !positionals.is_empty() {
            output.push_str("Arguments:\n\n");
            for arg in &positionals {
                let label = format_positional_label(arg);
                output.push_str(&format!("- `{}`", label));
                if let Some(help) = arg.get_help().or_else(|| arg.get_long_help()) {
                    output.push_str(&format!(" {help}"));
                }
                output.push('\n');
            }
            output.push('\n');
        }

        if !options.is_empty() {
            output.push_str("Options:\n\n");
            for arg in &options {
                if let Some(label) = format_option_label(arg) {
                    output.push_str(&format!("- `{}`", label));
                    if let Some(help) = arg.get_help().or_else(|| arg.get_long_help()) {
                        output.push_str(&format!(" {help}"));
                    }
                    output.push('\n');
                }
            }
            output.push('\n');
        }
    }

    output
}

fn format_positional_label(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .map(|names| names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| arg.get_id().as_str().to_string());

    if arg.is_required_set() {
        name
    } else {
        format!("[{name}]")
    }
}

fn format_option_label(arg: &Arg) -> Option<String> {
    let mut flags = Vec::new();
    if let Some(short) = arg.get_short() {
        flags.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        flags.push(format!("--{long}"));
    }
    if flags.is_empty() {
        return None;
    }

    let mut label = flags.join(", ");
    if let Some(value_name) = arg
        .get_value_names()
        .map(|names| names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(" "))
    {
        label.push_str(&format!(" <{value_name}>"));
    }

    Some(label)
}

// ---------------------------------------------------------------------------
// MCP docgen
// ---------------------------------------------------------------------------

/// A single MCP tool's documentation entry.
#[derive(Clone)]
pub struct ToolDoc {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamDoc],
}

/// A single MCP tool parameter's documentation entry.
pub struct ParamDoc {
    pub name: &'static str,
    pub type_name: &'static str,
    pub required: bool,
    pub description: &'static str,
}

/// Generate MCP tools reference documentation as Markdown.
///
/// Takes a slice of tool documentation entries and produces a
/// Markdown-formatted string sorted alphabetically by tool name.
pub fn generate_mcp_docs(tools: &[ToolDoc]) -> String {
    let mut output = String::from("# 🤖 MCP tools\n\n");

    let mut tools = tools.to_vec();
    tools.sort_by(|a, b| a.name.cmp(b.name));

    for tool in tools {
        output.push_str(&format!("## `{}`\n\n", tool.name));
        output.push_str(tool.description);
        output.push_str("\n\n");

        if tool.params.is_empty() {
            output.push_str("Parameters: none\n\n");
            continue;
        }

        output.push_str("Parameters:\n\n");
        for param in tool.params {
            let requirement = if param.required { "required" } else { "optional" };
            output.push_str(&format!(
                "- `{}` ({}, {}) {}\n",
                param.name, param.type_name, requirement, param.description
            ));
        }
        output.push('\n');
    }

    output
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_cli_docs_basic() {
        use clap::{Command, Arg};

        let cmd = Command::new("test-cli")
            .subcommand(
                Command::new("subcommand1")
                    .about("First subcommand")
                    .arg(
                        Arg::new("input")
                            .required(true)
                            .value_name("INPUT")
                            .help("Input file"),
                    )
            )
            .subcommand(
                Command::new("subcommand2")
                    .about("Second subcommand")
                    .arg(
                        Arg::new("output")
                            .short('o')
                            .long("output")
                            .value_name("FILE")
                            .help("Output file"),
                    )
            );

        let docs = generate_cli_docs(&cmd);
        assert!(docs.contains("# 👩‍💻 Command line interface"));
        assert!(docs.contains("## `subcommand1`"));
        assert!(docs.contains("## `subcommand2`"));
        assert!(docs.contains("First subcommand"));
        assert!(docs.contains("Second subcommand"));
        assert!(docs.contains("Arguments:"));
        assert!(docs.contains("Options:"));
        assert!(docs.contains("`INPUT`"));
        assert!(docs.contains("`-o, --output <FILE>`"));
        assert!(docs.contains("Input file"));
        assert!(docs.contains("Output file"));
    }

    #[test]
    fn test_format_positional_label_required() {
        use clap::Arg;

        let arg = Arg::new("input").required(true);
        assert_eq!(format_positional_label(&arg), "input");
    }

    #[test]
    fn test_format_positional_label_optional() {
        use clap::Arg;

        let arg = Arg::new("input");
        assert_eq!(format_positional_label(&arg), "[input]");
    }

    #[test]
    fn test_format_option_label() {
        use clap::Arg;

        let arg = Arg::new("output")
            .short('o')
            .long("output")
            .value_name("FILE");
        assert_eq!(format_option_label(&arg), Some("-o, --output <FILE>".to_string()));
    }

    #[test]
    fn test_format_option_label_long_only() {
        use clap::Arg;

        let arg = Arg::new("verbose")
            .long("verbose");
        assert_eq!(format_option_label(&arg), Some("--verbose".to_string()));
    }

    #[test]
    fn test_format_option_label_short_only() {
        use clap::Arg;

        let arg = Arg::new("help")
            .short('h');
        assert_eq!(format_option_label(&arg), Some("-h".to_string()));
    }

    #[test]
    fn test_generate_mcp_docs_basic() {
        let tools = vec![
            ToolDoc {
                name: "search",
                description: "Search the web.",
                params: &[
                    ParamDoc {
                        name: "query",
                        type_name: "string",
                        required: true,
                        description: "Search query.",
                    },
                ],
            },
            ToolDoc {
                name: "fetch",
                description: "Fetch a URL.",
                params: &[],
            },
        ];

        let docs = generate_mcp_docs(&tools);
        assert!(docs.contains("# 🤖 MCP tools"));
        assert!(docs.contains("## `fetch`"));
        assert!(docs.contains("## `search`"));
        assert!(docs.contains("Search the web."));
        assert!(docs.contains("Fetch a URL."));
        assert!(docs.contains("Parameters: none"));
        assert!(docs.contains("`query` (string, required) Search query."));
    }
}
