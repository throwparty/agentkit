use std::time::Duration;

/// Resource bounds applied to a [`crate::ScriptEngine`].
///
/// Every limit is defensive: a script that exceeds any of them is terminated
/// with a [`crate::ScriptError::LimitExceeded`] rather than being allowed to
/// consume unbounded host resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Maximum number of Rhai operations before termination.
    pub max_operations: u64,
    /// Maximum function/closure call nesting depth.
    pub max_call_levels: usize,
    /// Maximum expression nesting depth.
    pub max_expr_depth: usize,
    /// Maximum expression nesting depth per function call.
    pub max_function_expr_depth: usize,
    /// Maximum size of a single string, in bytes.
    pub max_string_size: usize,
    /// Maximum number of elements in a single array.
    pub max_array_size: usize,
    /// Maximum number of entries in a single object map.
    pub max_map_size: usize,
    /// Maximum number of interned strings.
    pub max_strings_interned: usize,
    /// Wall-clock budget for a single script invocation.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 100_000,
            max_call_levels: 50,
            max_expr_depth: 64,
            max_function_expr_depth: 32,
            max_string_size: 1_048_576,
            max_array_size: 10_000,
            max_map_size: 10_000,
            max_strings_interned: 1_024,
            timeout: Duration::from_millis(100),
        }
    }
}
