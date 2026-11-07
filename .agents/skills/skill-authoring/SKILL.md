---
name: skill-authoring
description: Authoring opencode skills for this project
---

## When to use me

When creating or modifying skills in `.agents/skills/`.

## How skills work

Skills are markdown files at `.agents/skills/<name>/SKILL.md` that provide structured instructions for opencode agents. They are loaded on demand via the `skill` tool when a task matches the skill's `description` or when the agent is explicitly directed to use them.

## SKILL.md format

Each skill is a single `SKILL.md` file with YAML frontmatter and markdown body.

### Frontmatter

```yaml
---

name: <short-name>
description: <one-line description of when to use>

---

```

Required fields:

- `name` — kebab-case identifier, unique within the project
- `description` — concise sentence telling an agent when this skill applies

No other frontmatter fields are needed for project-level skills.

### Body

Write in plain markdown. Structure as:

```markdown
## When to use me

Clear conditions for when an agent should load this skill. Be specific — the agent decides based on this description.

## Content

Write instructions, conventions, and examples. Use:

- Short sections with `##` headings
- Code blocks with language annotations
- Bullet lists for conventions
- Prefer concrete examples over abstract rules
```

## Capability prompts (Tackle)

Reusable knowledge for an agent is a Tackle prompt, not an opencode skill:
`.agents/` is gitignored here, so anything the whole repository shares must live
under `.agentkit/tackle/`. A capability prompt teaches a technology the model
cannot be trusted to know from its training: version-specific behaviour,
surprising interactions, and the commands that prove them. It differs from a
process prompt, which tells an agent what to do. Use one when the technology's
details are load-bearing for a component's behaviour.

### When a capability prompt is warranted

All four must hold:

1. A component's behaviour depends on the technology's details, not just its name.
1. The details are version-specific or fast-moving, so model priors are unreliable.
1. Each fact can be re-verified on a supported host with a cheap command.
1. There is a component here that owns the integration.

If the material is only vocabulary, write the package's `glossary.md` and skip
the prompt body.

### Layout

```
.agentkit/tackle/prompts/<technology>/
  PROMPT.md      # the entry point: behaviour, loaded on demand
  glossary.md    # a resource: the words, read when naming something
```

Frontmatter: a `description` that says when to load it, `model_invokable: true`,
and `user_invokable: true` so the slash command works. Resources are listed to
the model with the expanded body, so name them from the body via `{{ $.dir }}`.

### Sections

- `## When to use me` — the tasks that need it, and the glossary to read first.
- `## Mental model` — at most five numbered statements, in dependency order. This is what makes the tool predictable.
- `## Verified facts` — each fact, the command that produced it, the observed result, and the version and host it was verified on.
- `## Traps` — true but surprising; the things a confident wrong answer would get wrong.
- `## Re-verifying` — one runnable block that reproduces the facts.
- `## Where the truth lives` — the glossary, the upstream manual, and any in-repo evidence.
- `## What this does not cover` — the boundary, so the prompt does not imply completeness.

### Rules

- Every fact carries its proof. A fact that cannot be re-run does not belong.
- Record the version and host. Behaviour that changes under a different version
  is a trap, and the version is what makes the fact falsifiable.
- The glossary is a resource of the same package, not a copy in `CONTEXT.md`.
  It holds upstream words only; a term we coin, or a narrowing of an upstream
  word, lives in the owning component's `CONTEXT.md`.
- Keep it a subset of upstream. Link the manual for the rest.
- When a fact is refuted, correct it in place and note the version that changed
  it, rather than deleting the history from the traps.

The first instance is `.agentkit/tackle/prompts/nix/`, with its vocabulary in
`glossary.md` beside the prompt.

## Best practices

1. **Single concern** — each skill covers one topic.
1. **Project-specific** — capture conventions and patterns unique to this repo.
1. **Actionable** — write so an agent can follow the instructions without guessing.
1. **Concise** — short sections, minimal prose, lots of examples.
1. **Concrete** — show real file paths and patterns from this repo.
1. **Idempotent** — loading the same skill twice is harmless.
1. **No orchestration logic** — skills are reference material, not scripts.
