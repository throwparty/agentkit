//! Definitions: the persona, actor, and prompt types and their loading.
//!
//! Definitions are markdown files with frontmatter, discovered from the
//! repository tree (`.agentkit/tackle/<kind>/`). Frontmatter format is
//! marker-dispatched: `---` fences parse as YAML (the ecosystem convention,
//! so definitions lift in unchanged), `+++` fences parse as TOML. Parsing is
//! bounded by a size cap; validation errors name the file and, via the
//! deserialiser's message, the offending key.

mod types;

pub mod context;
pub mod loaders;

pub use loaders::discover;
pub use types::{ActorMeta, Definition, DefinitionError, Definitions, PersonaMeta, PromptMeta};
