# Lead: koto-skills plugin structure and koto-user placement

## Findings

### Plugin layout

The `koto-skills` plugin lives at `plugins/koto-skills/` with this structure:

```
plugins/koto-skills/
├── .claude-plugin/
│   └── plugin.json          # manifest: name, version, skills array
├── AGENTS.md                # runtime reference for agents using koto (mirrors koto-user need)
├── eval.sh                  # prompt regression eval harness
├── hooks.json               # Stop hook: reminds agent of active workflows
└── skills/
    └── koto-author/
        ├── SKILL.md
        ├── koto-templates/
        │   ├── koto-author.md
        │   └── koto-author.mermaid.md
        └── references/
            ├── template-format.md
            └── examples/
                ├── complex-workflow.md
                ├── complex-workflow.mermaid.md
                ├── evidence-routing-workflow.md
                └── evidence-routing-workflow.mermaid.md
```

### Manifest format

`plugin.json` at `plugins/koto-skills/.claude-plugin/plugin.json`:

```json
{
  "name": "koto-skills",
  "version": "0.5.1-dev",
  "description": "Workflow skills for koto ...",
  "author": {"name": "tsukumogami"},
  "skills": [
    "./skills/koto-author"
  ]
}
```

The `skills` field is an **array of paths**, one entry per skill directory. Adding
`koto-user` is a single line append: `"./skills/koto-user"`.

Compare with shirabe's `plugin.json`, which uses a directory glob `"skills": "./skills/"`.
koto-skills uses explicit enumeration — each skill is listed individually. Both approaches
are valid; the enumeration style means new skills don't auto-register and must be explicitly
added.

### Discovery mechanism

The marketplace is declared in `.claude-plugin/marketplace.json` at the repo root. It lists
`koto-skills` as a plugin with `"source": "./plugins/koto-skills"`. Plugin consumers install
via Claude Code's plugin mechanism, which reads `plugin.json` from the declared source path
and sets `CLAUDE_SKILL_DIR` to each skill's directory when loading.

The `hooks.json` at the plugin root (not inside a skill) provides a Claude Code `Stop` hook
that runs after every agent session and reminds the agent of active koto workflows. This hook
is plugin-level, shared across all skills.

### koto-author skill structure

`koto-author/SKILL.md` has:
- YAML frontmatter: `name`, `description`
- Usage block with `koto init` command using `${CLAUDE_SKILL_DIR}/koto-templates/koto-author.md`
- Reference material section pointing to `${CLAUDE_SKILL_DIR}/references/`
- 8-state workflow description and when-to-use guidance

The `references/` directory contains reusable template authoring docs (`template-format.md`,
examples). These are author-facing, not user-facing. A koto-user skill would not need these
references — its reference material would be the command contract (already fully documented
in `AGENTS.md` at the plugin root).

### What adding koto-user requires

Adding `koto-user` as a second skill in the same plugin involves:

1. Create `plugins/koto-skills/skills/koto-user/` with a `SKILL.md`
2. Optionally add `koto-templates/` if the skill is koto-backed
3. Add `"./skills/koto-user"` to the `skills` array in `plugin.json`
4. Optionally add eval cases to `evals/` (directory doesn't exist yet; eval.sh supports it)
5. CI (`validate-plugins.yml`) already finds templates via `find plugins/koto-skills/skills/ -path '*/koto-templates/*.md'` — no CI changes needed

No structural changes to the plugin itself are required. The Skills array, directory layout,
hook system, and CI pipelines are all already designed for multiple skills.

### AGENTS.md overlap

`plugins/koto-skills/AGENTS.md` is a 550-line plugin-level runtime reference covering every
`koto next` response shape, all error codes, and two worked examples (including the koto-author
workflow and the work-on workflow). This file is the primary reference for any agent *running*
a koto workflow — exactly the knowledge domain of koto-user.

The koto-user SKILL.md would not need to duplicate this content; it would reference or
incorporate AGENTS.md. The question is whether koto-user should point to AGENTS.md as its
reference material, inline the key sections, or restructure things so AGENTS.md becomes a
`references/` file owned by koto-user.

### Audience comparison

| Dimension | koto-author | koto-user |
|-----------|-------------|-----------|
| Persona | Skill developer writing new templates | Agent running an existing koto-backed workflow |
| Core task | Design states, write templates, validate structure | Init workflow, call koto next, dispatch on action, submit evidence |
| References needed | template-format.md, examples, compiler behavior | koto next response shapes, error codes, evidence submission |
| koto template used | Yes (8-state authoring workflow) | Possibly (could be a simple SKILL.md if the loop is clear) |
| Shared infrastructure | hooks.json (both benefit from active-workflow reminder) | Same hooks.json |

The audiences are different enough that a user landing on koto-author would not find what they
need, and vice versa. But they share:
- The Stop hook (both benefit from workflow resume reminders)
- The plugin installation path (consumers install one plugin, get both)
- The overall koto context (both require koto on PATH)

### Same plugin vs. separate plugin

Arguments for keeping them in the same plugin (`koto-skills`):
- Single install gives agents access to both author and user guidance
- The Stop hook is shared — no duplication
- `plugin.json` already supports multiple skills
- The pairing is natural: if you build a koto-backed skill, you'll use both at different times
- Naming stays coherent: "koto-skills" as a collection of koto-related skills

Arguments for a separate plugin:
- Cleaner installation story for consumers who only run workflows (don't author templates)
- Separation prevents koto-user consumers from being confused by koto-author content
- Would require creating a new marketplace entry and plugin directory

The separate-plugin path adds overhead with little benefit: consumers who run koto-backed
workflows are often working inside a project that's already installed koto-skills for authoring.
A second plugin would create two install steps for the same audience.

## Implications

Adding `koto-user` to the `koto-skills` plugin is mechanical. The manifest change is one line.
The CI is already wired to pick up new skills automatically. The Stop hook benefits both skills
without modification.

The main design decision is how koto-user relates to AGENTS.md. Three options:
1. koto-user SKILL.md inlines the key runtime loop content and treats AGENTS.md as supplementary
2. koto-user SKILL.md is minimal and points to AGENTS.md as its primary reference
3. Move AGENTS.md into `skills/koto-user/references/` and make koto-user the canonical runtime
   reference (AGENTS.md becomes an artifact of koto-user rather than the plugin root)

Option 3 is the most coherent long-term, but it changes where AGENTS.md lives and may affect
any tooling that copies AGENTS.md to a project root. Options 1 and 2 avoid that disruption.

## Surprises

`AGENTS.md` at the plugin root is already a nearly complete koto-user reference document.
It covers all response shapes, error codes, and two end-to-end workflow examples. The main
gap between AGENTS.md and a proper koto-user SKILL.md is the absence of:
- When-to-use framing (koto-user vs. manually reading templates)
- Prerequisites section
- Structured resume/interruption guidance
- Integration with the `${CLAUDE_SKILL_DIR}` convention for template paths

In other words, the content exists; it needs wrapping into SKILL.md conventions and some
audience framing.

The eval harness at `eval.sh` expects an `evals/` directory that doesn't exist yet. It
silently skips evaluation when the directory is absent. CI runs eval.sh but guards with
`if [ -z "$ANTHROPIC_API_KEY" ]` — evals are currently optional. Adding koto-user is an
opportunity to also add the first evals.

## Open Questions

1. Should AGENTS.md move to `skills/koto-user/references/` (making koto-user the canonical
   location for runtime reference content), or stay at the plugin root to preserve its
   role as a project-level drop-in file?

2. Should koto-user be a koto-backed SKILL.md (using a workflow template) or a plain SKILL.md?
   For a tool whose job is to run koto workflows, a koto-backed meta-skill is elegant but
   possibly overkill for what is essentially a short execution loop.

3. What eval cases should accompany koto-user? What behaviors are most likely to regress?
   (Candidate: does the agent correctly dispatch on `action` vs. just reading `directive`?)

4. Does the `plugin.json` `skills` enumeration need to stay explicit, or should it shift to
   directory glob (`"./skills/"`) as shirabe uses? The glob approach would make future skill
   additions zero-config.

## Summary

The koto-skills plugin already supports multiple skills — adding koto-user requires only a
directory under `skills/koto-user/` and one line in `plugin.json`, with no CI or hook changes
needed. The two skills have clearly different audiences (template authors vs. workflow runners),
but share enough infrastructure (hooks, plugin install, koto runtime context) that grouping
them in the same plugin is the right call. The biggest open question is whether AGENTS.md
should move to become a koto-user reference file or stay at the plugin root as a standalone
drop-in for projects that don't install the plugin.
