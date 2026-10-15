use std::path::Path;

use rhai::packages::Package;

use crate::error::ScriptError;
use crate::limits::Limits;
use crate::script::CompiledScript;

/// A sandboxed Rhai engine with no host capabilities.
///
/// The engine is built from [`rhai::Engine::new_raw`] plus the standard package,
/// so scripts get arrays, maps, strings and operators but no file module
/// resolver, no filesystem access and no network access. No host functions are
/// registered, so the only API a script sees is what its compiled JSON
/// arguments and return value provide. `eval`, `import` and `export` are
/// disabled, and every limit in [`Limits`] is enforced.
///
/// `ScriptEngine` is `Send + Sync` and intended to be shared as an
/// `Arc<ScriptEngine>`.
pub struct ScriptEngine {
    engine: rhai::Engine,
    limits: Limits,
}

impl std::fmt::Debug for ScriptEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptEngine")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptEngine {
    /// Create an engine with [`Limits::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// Create an engine with the supplied limits.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        let mut engine = rhai::Engine::new_raw();
        rhai::packages::StandardPackage::new().register_into_engine(&mut engine);
        engine.disable_symbol("eval");
        engine.disable_symbol("import");
        engine.disable_symbol("export");
        engine.set_max_operations(limits.max_operations);
        engine.set_max_call_levels(limits.max_call_levels);
        engine.set_max_expr_depths(limits.max_expr_depth, limits.max_function_expr_depth);
        engine.set_max_string_size(limits.max_string_size);
        engine.set_max_array_size(limits.max_array_size);
        engine.set_max_map_size(limits.max_map_size);
        engine.set_max_strings_interned(limits.max_strings_interned);

        Self { engine, limits }
    }

    /// The limits this engine enforces.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Compile `source` under `origin`.
    ///
    /// `origin` names the script in error messages; use a file path so failures
    /// can be traced back to the file the operator configured.
    pub fn compile(
        &self,
        origin: impl Into<String>,
        source: &str,
    ) -> Result<CompiledScript, ScriptError> {
        let origin = origin.into();
        self.engine
            .compile(source)
            .map(|ast| CompiledScript::new(std::sync::Arc::from(origin.as_str()), ast))
            .map_err(|error| ScriptError::Compile {
                origin,
                message: error.to_string(),
            })
    }

    /// Read and compile the script at `path`.
    pub fn compile_file(&self, path: &Path) -> Result<CompiledScript, ScriptError> {
        let source = std::fs::read_to_string(path).map_err(|source| ScriptError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        self.compile(path.display().to_string(), &source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INLINE_ORIGIN;

    #[test]
    fn compile_records_the_origin_and_defines_functions() {
        let engine = ScriptEngine::new();

        let script = engine
            .compile(INLINE_ORIGIN, "fn mutate(request) { request }")
            .unwrap();

        assert_eq!(script.origin(), INLINE_ORIGIN);
        assert!(script.defines("mutate", 1));
        assert!(!script.defines("mutate", 2));
    }

    #[test]
    fn eval_import_and_export_are_disabled() {
        let engine = ScriptEngine::new();

        for source in [
            "fn mutate(request) { eval(\"1 + 1\") }",
            "import \"some/module\" as m; fn mutate(request) { 1 }",
        ] {
            let error = engine.compile(INLINE_ORIGIN, source).unwrap_err();
            assert!(
                matches!(error, ScriptError::Compile { .. }),
                "{source} should fail to compile: {error:?}"
            );
        }
    }

    #[test]
    fn a_missing_file_is_a_read_error() {
        let engine = ScriptEngine::new();

        let error = engine
            .compile_file(std::path::Path::new("/nonexistent/nowhere.rhai"))
            .unwrap_err();

        assert!(matches!(error, ScriptError::Read { .. }), "{error:?}");
    }

    #[test]
    fn compile_file_uses_the_path_as_the_origin() {
        let dir = std::env::temp_dir().join(format!("agentkit-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mutate.rhai");
        std::fs::write(&path, "fn mutate(request) { request }").unwrap();

        let script = ScriptEngine::new().compile_file(&path).unwrap();
        assert_eq!(script.origin(), path.display().to_string());

        std::fs::remove_dir_all(&dir).ok();
    }
}
