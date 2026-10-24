# definitions

Persona, actor, and prompt definitions: the types and their loading.

## What lives here

```
mod.rs       module doc + re-exports (Definition, Definitions,
             DefinitionError, the frontmatter metas, discover)
types.rs     the types: Definition<T>, Definitions, DefinitionError,
             PersonaMeta, ActorMeta, PromptMeta, and the collection's
             pure operations (trust entries, project retention)
loaders.rs   discovery across the user and project layers,
             marker-dispatched frontmatter parsing (--- YAML / +++ TOML),
             built-in registration, size-capped reads
```

A definition is a markdown file with frontmatter: the file stem is its
name, the frontmatter is typed metadata, the body is the persona's
behavioural prompt or the prompt's expansion template.

## Dependencies

- depends on: `config::trust` (content hashing for TOFU records),
  `builtins` (the default definitions registered at lowest precedence)
- depended on by: `main` (startup discovery), `config` (project trust
  entries), `invokables` (the registry), `acp::v1` (compaction prompts)
