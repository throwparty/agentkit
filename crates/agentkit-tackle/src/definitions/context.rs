//! Harness-provided context variables for definition bodies.
//!
//! The `$` namespace is reserved: `{{ $.name }}` is the definition name and
//! `{{ $.dir }}` its repository-relative package directory. The sigil keeps
//! them distinct from declared parameters, which stay bare (`{{ dir }}`).
//! Substitution is shared by prompt expansion and persona system-prompt
//! assembly so the two cannot diverge.

use std::collections::BTreeMap;
use std::path::Path;

/// The `$` namespace values for a definition. A flat definition has no
/// directory, so `$.dir` is empty.
pub fn variables(name: &str, directory: Option<&Path>) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("$.name".to_owned(), name.to_owned()),
        (
            "$.dir".to_owned(),
            directory
                .map(|directory| directory.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default(),
        ),
    ])
}

/// Substitutes `{{ name }}` placeholders (inner whitespace tolerated) with
/// the declared parameters first, then the `$` namespace. Unknown
/// placeholders are left as written.
pub fn render(
    body: &str,
    parameters: &BTreeMap<String, String>,
    name: &str,
    directory: Option<&Path>,
) -> String {
    let mut values = parameters.clone();
    values.extend(variables(name, directory));
    substitute(body, &values)
}

/// Substitutes placeholders from `values`; `{{ key }}` and `{{key}}` are
/// equivalent.
pub fn substitute(body: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                match values.get(key) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn context_variables_substitute() {
        let directory = PathBuf::from(".agentkit/tackle/prompts/deploy");
        let rendered = render(
            "name={{ $.name }} dir={{ $.dir }}",
            &BTreeMap::new(),
            "deploy",
            Some(&directory),
        );
        assert_eq!(rendered, "name=deploy dir=.agentkit/tackle/prompts/deploy");
    }

    #[test]
    fn a_parameter_named_dir_does_not_collide() {
        let parameters = BTreeMap::from([("dir".to_owned(), "/tmp".to_owned())]);
        let directory = PathBuf::from(".agentkit/tackle/prompts/deploy");
        let rendered = render(
            "param={{ dir }} context={{ $.dir }}",
            &parameters,
            "deploy",
            Some(&directory),
        );
        assert_eq!(
            rendered,
            "param=/tmp context=.agentkit/tackle/prompts/deploy"
        );
    }

    #[test]
    fn a_flat_definition_yields_an_empty_directory() {
        let rendered = render("dir={{ $.dir }}", &BTreeMap::new(), "flat", None);
        assert_eq!(rendered, "dir=");
    }
}
