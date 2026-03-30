---
status: Proposed
problem: |
  There's no guided way to create a koto-backed skill. Template authoring requires
  understanding the YAML frontmatter schema, state machine semantics, evidence routing
  constraints, and the file-system coupling convention (koto-templates/ directory,
  ${CLAUDE_SKILL_DIR} references, SHA-256 hash-lock). Today this is done manually by
  reading docs and existing templates. A skill that guides Claude Code agents through
  authoring both the SKILL.md and its bundled koto template would lower the barrier
  and produce more consistent output.
---

# DESIGN: Koto template authoring skill

## Status

Proposed

## Context and problem statement

Koto is a workflow orchestration engine that uses markdown templates with YAML
frontmatter to define state machines. Skills are Claude Code's mechanism for
packaging reusable agent behaviors. Several skills already use koto templates as
their execution engine (e.g., work-on, explore), but creating new koto-backed
skills is a manual process that requires understanding multiple systems:

- The koto template format (states, transitions, evidence routing, gates, variables)
- The skill format (SKILL.md structure, reference files, agent prompts)
- The coupling convention (koto-templates/ subdirectory, ${CLAUDE_SKILL_DIR} paths)
- The distribution model (marketplace plugins)
- Compile-time validation rules (13+ checks including evidence routing mutual exclusivity)

The skill-creator skill exists for authoring generic Claude Code skills, but it
doesn't know about koto templates. We need a koto-native equivalent that produces
both the skill definition (what to do) and the template (how to do it) as a paired
unit.

The shirabe koto adoption PRD (tsukumogami/shirabe#48) provides immediate context:
7 existing skills need koto template conversions. Each follows the same pattern --
extract phases from prose SKILL.md, encode as koto states with gates and transitions,
maintain separation of concerns (SKILL.md = what to achieve, template = how to get
there). The PRD documents that 60-80% of current SKILL.md line counts are duplicated
workflow boilerplate (resume logic, phase ordering, gate checks), and that this
duplication causes real failures: phase skipping, lost decisions, brittle resume.
These 7 conversions are the immediate use case for this skill.

## Decision drivers

- The skill must teach agents how to write valid koto templates, not just generate them blindly
- Template validation should use `koto template compile` as a mechanical check
- The output must follow the established coupling convention (koto-templates/ directory)
- Distribution should use koto's existing marketplace infrastructure
- The teaching approach should layer concepts: linear workflows first, then evidence routing, then advanced features
- The skill-creator's workflow pattern (capture intent, draft, validate, iterate) is proven and should be adapted
- The shirabe koto adoption PRD identifies 7 skills needing conversion, establishing concrete patterns: gate selection (7+ types), fan-out collection (parallel agents + glob gate), graceful degradation (skills must work without koto), and phase decomposition from existing prose
- The skill should be aware of koto's evolving feature surface -- templates shouldn't depend on unshipped features without flagging it

## Decisions already made

These choices were settled during exploration and should be treated as constraints:

- **Validation via `koto template compile`, not eval agents**: the compiler is deterministic and catches structural errors. Agent-based evaluation (like the skill-creator's grader) adds complexity for uncertain benefit in v1.
- **Distribution via koto's existing marketplace as a new plugin**: infrastructure exists. Standalone SKILL.md files lack versioning and discoverability.
- **Layered teaching approach**: linear workflows -> evidence routing -> advanced features (gates, variables, integration). Evidence routing mutual exclusivity is the key non-obvious constraint.
- **Templates always bundled with skills**: the output is always a paired SKILL.md + koto-templates/<name>.md. Decoupled templates are out of scope.
