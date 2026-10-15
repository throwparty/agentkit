use std::sync::Arc;

use rhai::AST;

/// A script compiled by a [`crate::ScriptEngine`].
///
/// Cloning is cheap: the compiled AST and origin are reference counted, so the
/// same script can be shared across threads and stored where it is used.
#[derive(Debug, Clone)]
pub struct CompiledScript {
    origin: Arc<str>,
    ast: AST,
}

impl CompiledScript {
    pub(crate) fn new(origin: Arc<str>, ast: AST) -> Self {
        Self { origin, ast }
    }

    /// Where this script came from: a file path, or `<inline>`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether the script defines a global `fn <name>` taking `arity` arguments.
    pub fn defines(&self, function: &str, arity: usize) -> bool {
        self.ast.iter_functions().any(|definition| {
            definition.name == function
                && definition.params.len() == arity
                && definition.this_type.is_none()
        })
    }

    pub(crate) fn ast(&self) -> &AST {
        &self.ast
    }

    pub(crate) fn origin_owned(&self) -> String {
        self.origin.to_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::ScriptEngine;

    #[test]
    fn defines_reports_arity_and_this_type() {
        let engine = ScriptEngine::new();
        let script = engine
            .compile("inline", "fn mutate(request) { request } fn helper() { 1 }")
            .unwrap();

        assert!(script.defines("mutate", 1));
        assert!(!script.defines("mutate", 0));
        assert!(script.defines("helper", 0));
        assert!(!script.defines("helper", 1));
        assert!(!script.defines("missing", 1));
    }

    #[test]
    fn origin_reports_the_compile_origin() {
        let engine = ScriptEngine::new();
        let script = engine
            .compile("some/where.rhai", "fn mutate(request) { request }")
            .unwrap();

        assert_eq!(script.origin(), "some/where.rhai");
    }
}
