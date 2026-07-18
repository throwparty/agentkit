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

## Best practices

1. **Single concern** — each skill covers one topic.
2. **Project-specific** — capture conventions and patterns unique to this repo.
3. **Actionable** — write so an agent can follow the instructions without guessing.
4. **Concise** — short sections, minimal prose, lots of examples.
5. **Concrete** — show real file paths and patterns from this repo.
6. **Idempotent** — loading the same skill twice is harmless.
7. **No orchestration logic** — skills are reference material, not scripts.
