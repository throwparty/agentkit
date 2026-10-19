# builtins

Built-in default assets, embedded in the binary.

## What lives here

```
mod.rs   the embedded assets and `defaults()`: the default persona,
         the default actor, the shipped titling and compaction scripts,
         the /fork fallback prompt, and the no-tools summariser actor
*.md     the asset bodies (personas, actors, scripts, prompts)
```

Built-ins are registered at the **lowest precedence** through the same
discovery path as user definitions: any discovered definition with the
same name replaces the built-in, and the replacement is surfaced as a
`builtin_override` at first run. The shipped scripts double as the
customisation examples.

## Dependencies

- depends on: nothing (embedded strings)
- depended on by: `definitions` (registered beneath discovered
  definitions), `config` (the default script registrations)
