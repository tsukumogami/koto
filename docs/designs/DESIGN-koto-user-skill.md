---
status: Planned
upstream: docs/prds/PRD-koto-user-skill.md
problem: |
  The koto-skills plugin has no skill covering the agent runtime loop. koto-author
  covers template authoring, but an agent running a koto-backed workflow has no
  installable guidance for init, next-dispatch, evidence submission, override flow,
  or rewind. Separately, koto-author has accumulated four documentation gaps since
  structured gate output shipped, and two phantom CLI references that send agents
  to commands that don't exist. A root AGENTS.md for the koto repo is also absent,
  leaving Claude Code sessions without basic orientation.
decision: |
  Create a koto-user skill in the existing koto-skills plugin with a SKILL.md
  covering the full runtime loop and three reference files (command-reference.md,
  response-shapes.md, error-handling.md). Migrate the existing plugins/koto-skills/
  AGENTS.md content into these reference files and delete the original. Update
  koto-author in place by extending template-format.md and the existing example
  files. Create AGENTS.md at the koto repo root for session orientation.
rationale: |
  Placing koto-user in the existing plugin avoids a split install step with no
  benefit. Reference files linked from SKILL.md are the correct mechanism for
  deep skill content -- not AGENTS.md, which Claude Code auto-loads by directory
  rather than on demand. Updating koto-author in existing files preserves its
  structure. The root AGENTS.md solves the directory-scoping problem that makes
  the current plugin-buried AGENTS.md invisible to most sessions.
---

# DESIGN: koto-user skill and koto-skills plugin update

## Status

Planned

## Context and Problem Statement

The `koto-skills` plugin provides Claude Code skills for agents working with koto
workflows, but it only covers one of two agent personas. An agent *authoring* a
template can install `koto-author` and get structured guidance. An agent *running*
a workflow has nothing: no guidance on interpreting `koto next` output, dispatching
on `action` values, submitting evidence, handling blocked gates, or rewinding states.

This gap compounded when koto shipped structured gate output across PRs #120-#125.
`koto-author` was not updated, so it now documents a legacy gate pattern that fails
`koto template compile` in strict mode, omits the override mechanism entirely, and
references two commands (`koto status`, `koto query`) that don't exist in the CLI.

Three implementation problems need solving:

1. **New skill directory**: `plugins/koto-skills/skills/koto-user/` must be wired
   into `plugin.json` and contain a `SKILL.md` plus three reference files with
   precise content boundaries.

2. **koto-author in-place updates**: Four content gaps across `template-format.md`
   and the example files must be filled without restructuring the skill.

3. **Root orientation file**: `AGENTS.md` at the koto repo root is the only file
   Claude Code reliably auto-loads for any session in this directory. The existing
   `plugins/koto-skills/AGENTS.md` is scoped to the plugin directory and invisible
   to most sessions; it must be deleted after its content migrates to koto-user's
   reference files.

## Decision Drivers

- **Content accuracy**: all CLI commands, flags, and response schemas documented
  must match the current Rust source (`src/cli/`, `src/engine/`, `src/gate/`)
- **Navigation cost**: agents should reach what they need in one or two file reads,
  not by chasing a chain of references
- **Bounded scope**: root AGENTS.md is orientation only (≤80 lines); depth lives
  in skill reference files
- **No plugin split**: koto-user and koto-author stay in the same plugin to share
  the install step
- **koto-author structure preserved**: updates extend existing sections rather than
  adding new reference files

## Considered Options

### Decision 1: responsibility split between SKILL.md and reference files

The koto-user skill splits its domain content across a main SKILL.md and three
reference files. The question is where the line falls. Three approaches were
evaluated.

Key assumptions: reference files are not auto-loaded; agents follow links only
when they explicitly need depth. The typical session reaches `evidence_required`
on most agent-facing states, making evidence submission the most frequently needed
pattern.

#### Chosen: balanced SKILL.md

SKILL.md covers the full runtime lifecycle, the 6-value action dispatch table,
the three `evidence_required` sub-cases with their distinguishing signals, the
evidence submission pattern inline (`koto next <name> --with-data '<json>'`), the
two-step override flow, and links to the three reference files.

The reference files carry depth: command-reference.md has every subcommand with
full flag documentation; response-shapes.md has annotated JSON examples for all
6 action values; error-handling.md covers exit codes 0-3 and the
`agent_actionable: false` scenario.

The three `evidence_required` sub-cases live in SKILL.md rather than
response-shapes.md because they determine which action path the agent takes.
Getting them wrong causes an agent to submit evidence when it should first address
blocking gates — this is correctness-critical, not just depth.

#### Alternatives considered

**Thin SKILL.md** — lifecycle and dispatch table only; evidence submission in
response-shapes.md. Rejected: the `--with-data` pattern is needed on almost every
loop iteration. Forcing agents to open a reference file for it violates the
navigation cost constraint.

**Fat SKILL.md** — SKILL.md reproduces the full AGENTS.md content (all schemas,
all error codes, full command reference). Rejected: recreates the monolith problem
and makes it harder for agents to find specific information. Reference files become
redundant.

**Single reference file** — merge command-reference.md, response-shapes.md, and
error-handling.md into one document. Rejected: the three topics are consulted in
different contexts (flag syntax vs. action schema vs. exit code diagnosis). A merged
file makes each lookup longer. Three focused files are cheaper to navigate than one
long one.

---

### Decision 2: gate documentation format in koto-author template-format.md

Layer 3 of `template-format.md` already introduces gate types via a summary table
but doesn't document the `gates.<name>.<field>` path syntax, per-gate output
fields, or `override_default`. Template authors writing `when`-clauses need to
know exactly which field names to reference for each gate type.

Key assumption: context-matches output fields are provisional and marked as such.
The existing overview table remains; new content extends it with per-gate
subsections.

#### Chosen: annotated YAML examples with embedded per-gate field tables

Each gate type gets a subsection with a compact field table (name, type,
description) followed by a complete annotated YAML block showing the gate
declaration and a `when`-block using the emitted fields. `override_default` appears
in the annotated YAML with an inline comment explaining the three-tier resolution
order (`--with-data` > `override_default` > built-in default).

Template authors need patterns to copy, not schemas to interpret. An annotated
example answers "which field name?" and "what syntax?" simultaneously. The embedded
table provides a scannable cross-reference for authors who already know the pattern.
Both are needed; neither alone is sufficient.

#### Alternatives considered

**Per-gate output tables only** — clean field reference but forces authors to
mentally compose the field path syntax from the table. The connection between "field
name in table" and `gates.<name>.<field>` path is non-obvious, especially since
Layer 2 `when`-blocks use plain evidence field names, not gate paths. Rejected.

**Prose descriptions with inline schema blocks** — harder to scan under time
pressure. Template authors skip prose and look for code blocks. Spreads information
across more vertical space without the density of a table. Rejected.

---

### Decision 3: root AGENTS.md content selection

The root `AGENTS.md` will be auto-loaded by every Claude Code session in the koto
directory. It replaces the 550-line `plugins/koto-skills/AGENTS.md` (plugin-scoped,
invisible to most sessions) with a deliberate 80-line orientation file.

Key assumption: a one-sentence conceptual framing is adequate for cold-start
agents; the command table communicates what koto does through its verbs.

#### Chosen: command quick-reference table + skill pointers

A single orientation sentence, a compact table of the five required commands with
one-line descriptions, a "which skill to use" routing heuristic (2-4 lines), and
skill pointers plus the docs link. This uses roughly 40-50 lines — well inside
budget — and communicates koto's execution model through its command vocabulary
more precisely than prose.

#### Alternatives considered

**Conceptual overview + skill pointers** — allocates 15-25 lines to prose before
reaching commands. Uses more budget to communicate the same information less
precisely. Agents needing deeper conceptual grounding should follow skill pointers,
not read more AGENTS.md prose. Rejected.

**Minimal prose + skill pointers only** — fails to route agents cleanly to
koto-author vs. koto-user without additional prose that erases its compactness
advantage. An agent that doesn't know which skill applies is not oriented. Rejected.

---

### Decision 4: skill-bundled content vs. reference to docs/

The koto repo has a `docs/` folder with human-facing guides (e.g.,
`docs/guides/cli-usage.md`). The skill reference files covering the same domain
(command syntax, response schemas, error codes) could reference that folder instead
of bundling their own content.

#### Chosen: separate bundled documents with last-resort docs/ pointer

Skill reference files (`command-reference.md`, `response-shapes.md`,
`error-handling.md`) are distinct documents from anything in `docs/`. They cover the
same facts but are shaped differently: dispatch tables, annotated JSON examples, and
exit-code decision trees — formats optimized for an agent consulting a reference
mid-loop, not for a human learning the tool.

Each reference file ends with a single last-resort pointer to `docs/guides/cli-usage.md`
for cases the bundled content doesn't cover. This is a safety net for gaps, not a
primary path. The bundled files remain authoritative; `docs/` is only consulted when
an agent hits something not represented in the reference file at all.

This differs from the tsuku approach, where `docs/` guides are the intentional
deep-dive path for known complex topics. koto's reference surface is compact and
schema-focused — the bundled files cover the domain fully for the expected use cases.
The docs/ pointer handles the unexpected.

This matches the industry pattern: MCP servers bundle their schemas inline, Cursor
rules live in `.cursorrules`, GitHub Copilot instructions live in
`.github/copilot-instructions.md`. Agent-facing content is consistently bundled and
separately maintained from human-facing docs.

#### Alternatives considered

**Reference docs/ from skills (primary path)** — skill files link to
`docs/guides/cli-usage.md` as the main depth mechanism instead of bundling reference
content. Rejected: (1) staleness is a process problem addressed by the CLAUDE.md
protocol, not a co-location problem; (2) agents mid-loop should not navigate to a
human-facing guide with a different reading order.

**Generate skill content from docs/** — treat `docs/` as the source of truth and
derive skill reference files from it. Rejected: the documents are different shapes.
`cli-usage.md` explains how to use koto; `command-reference.md` is a flag-lookup
table. There is no mechanical transform between explanation and dispatch table.

**No docs/ pointer at all** — fully self-contained with no reference to docs/.
Rejected: leaves agents with no escape hatch when the bundled reference doesn't
cover an edge case. A single last-resort line costs nothing and prevents dead ends.

## Decision Outcome

The four decisions interlock cleanly. A balanced SKILL.md for koto-user gives
workflow agents everything they need for a typical session without opening reference
files — satisfying the navigation cost driver. Annotated YAML examples in
koto-author template-format.md give template authors copyable patterns, filling the
four documentation gaps without restructuring the skill. The root AGENTS.md routes
any session to the right skill via a command table and a short heuristic, using
roughly half its line budget. Skill reference files are standalone bundled documents optimized for agent
consumption, with a last-resort pointer to `docs/guides/cli-usage.md` at the end
of each file as a safety net for gaps.

The choices reinforce each other: D3 (command table) relies on koto-user providing
sufficient depth for workflow runners, which D1 (balanced SKILL.md) delivers. D2
operates independently on koto-author but shares the same "examples over prose"
reasoning as D1's dispatch table inline in SKILL.md. D4 keeps skill content primary
and self-contained while acknowledging that `docs/` exists as a fallback — not a
primary reference path, but an escape hatch for the unexpected.

## Solution Architecture

### Overview

The work produces three parallel outputs: a new koto-user skill directory wired
into the existing plugin, in-place updates to the koto-author skill across four
files, and a new root `AGENTS.md` paired with deletion of the plugin-buried one.
No new plugin, no new registry, no new infrastructure — just file additions,
edits, and one deletion.

### Components

```
plugins/koto-skills/
├── .claude-plugin/
│   └── plugin.json            ← add "./skills/koto-user" to skills array
├── AGENTS.md                  ← DELETE (content migrates to koto-user references/)
└── skills/
    ├── koto-author/           ← UPDATED IN PLACE
    │   ├── SKILL.md           ← remove koto status/koto query, add blocking_conditions schema
    │   ├── references/
    │   │   ├── template-format.md  ← extend Layer 3: gate output schemas + annotated examples
    │   │   └── examples/
    │   │       └── complex-workflow.md  ← update to gates.* routing
    │   └── koto-templates/
    │       └── koto-author.md      ← update compile_validation gate to gates.template_exists.exists
    └── koto-user/             ← NEW
        ├── SKILL.md
        └── references/
            ├── command-reference.md
            ├── response-shapes.md
            └── error-handling.md

AGENTS.md                      ← NEW at koto repo root
```

### Key interfaces

**plugin.json → skills**: the `skills` array in `.claude-plugin/plugin.json` is
the install mechanism. Adding `"./skills/koto-user"` makes the skill installable.
No other wiring is needed.

**SKILL.md → references/**: explicit markdown links are the only connection. The
three reference files are standalone documents; SKILL.md links to each with a
one-line description of when to follow it:
- `[Command reference](references/command-reference.md)` — full flag docs for all subcommands
- `[Response shapes](references/response-shapes.md)` — annotated JSON for all 6 action values
- `[Error handling](references/error-handling.md)` — exit codes and `agent_actionable: false`

**AGENTS.md → skills**: plain prose references by skill name. Agents reading
AGENTS.md learn that `koto-author` and `koto-user` exist and which one to install.

**koto-author SKILL.md → template-format.md**: already linked. The extension to
Layer 3 is additive; existing links remain valid.

### Content contracts

**koto-user SKILL.md** must contain:
- Session lifecycle: `koto init <name> --template <path>` → `koto next <name>` loop → done
- Action dispatch table: all 6 values with one-liner agent behavior for each
- Three `evidence_required` sub-cases and their distinguishing signals
- Evidence submission pattern: `koto next <name> --with-data '<json>'` with a one-line example
- Two-step override flow: `koto overrides record <name> --gate <gate> --rationale <text>`, then re-query
- Links to the three reference files

**koto-user references/command-reference.md** must include `koto context`
subcommands (`add`, `get`, `exists`, `list`) alongside the workflow-runner commands
listed in PRD R17. Agents hitting a blocking `context-exists` gate need to know how
to resolve it; omitting these commands leaves a gap in the recovery path. Each file
ends with: *"For topics not covered here, see `docs/guides/cli-usage.md`."*

**koto-user references/response-shapes.md** scenario coverage criteria — each
annotated JSON example must represent a distinct, meaningful agent decision point,
not just a structural variation. Required scenarios:
- `evidence_required` — sub-case (a): clean state, agent submits evidence
- `evidence_required` — sub-case (b): gate failed, accepts block present, agent can submit or override
- `evidence_required` — sub-case (c): both empty, auto-advance candidate
- `gate_blocked` — `agent_actionable: true`, override is possible
- `gate_blocked` — `agent_actionable: false`, agent cannot unblock
- `integration` — external integration running, agent waits
- `integration_unavailable` — integration not reachable, agent decides whether to override
- `done` — terminal state
- `confirm` — agent must confirm before transition

The sub-case (b) and `gate_blocked` with `agent_actionable: false` scenarios are the
correctness-critical ones; skipping them in favor of simpler examples would leave the
most error-prone cases undocumented.

**koto-author template-format.md Layer 3 extension** must contain per gate type:
- Field table: name, type, description for each output field
- Annotated YAML block: gate declaration + `when`-block using those fields
- `override_default` inline in the annotated YAML with three-tier resolution comment

The exact field names must be verified against `src/gate/` source
(`StructuredGateResult` and per-gate result types) before writing the documentation.
This is a required pre-step for Phase 1, not something that can be drafted from
memory.

## Implementation Approach

The three workstreams are independent and can be delivered in any order. The
suggested sequence delivers the highest-value fix first (koto-author phantom
commands are live bugs) and ensures koto-user exists before the old plugin
AGENTS.md is deleted.

### Phase 1: koto-author corrections

Fix the live bugs and documentation gaps in the existing skill.

Deliverables:
- `plugins/koto-skills/skills/koto-author/SKILL.md` — remove `koto status` and
  `koto query`; add `blocking_conditions` item schema (name, type, status,
  agent_actionable, output)
- `plugins/koto-skills/skills/koto-author/references/template-format.md` — extend
  Layer 3 with per-gate output tables and annotated YAML examples; document
  `override_default` and `koto overrides record/list`; document
  `--allow-legacy-gates` flag and D5 diagnostic
- `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md`
  — update gate-bearing states to `gates.*` routing; verify `koto template compile`
  exits 0 without `--allow-legacy-gates`
- `plugins/koto-skills/koto-templates/koto-author.md` — add D5 to compile error
  list; verify whether the `compile_validation` gate needs `gates.template_exists.exists: true`
  routing or already achieves the same result through evidence submission (read the
  template source before making this change — the PRD specifies it but the current
  template may already route correctly via a different mechanism)

### Phase 2: koto-user skill creation

Create the new skill directory and all its content.

Deliverables:
- `plugins/koto-skills/skills/koto-user/SKILL.md`
- `plugins/koto-skills/skills/koto-user/references/command-reference.md`
- `plugins/koto-skills/skills/koto-user/references/response-shapes.md`
- `plugins/koto-skills/skills/koto-user/references/error-handling.md`
- `plugins/koto-skills/.claude-plugin/plugin.json` — add `./skills/koto-user`

### Decision 4: single PR vs. separate PRs

The three phases are independent and could land separately. A single PR was chosen
because the cross-references between workstreams (AGENTS.md names both skills,
koto-user SKILL.md links to reference files, koto-author and koto-user share the
plugin) are easier to verify atomically. The feature ships as a unit.

### Phase 3: root AGENTS.md and plugin AGENTS.md deletion

Deliverables:
- `AGENTS.md` at koto repo root — command table, skill pointers, docs link
- Delete `plugins/koto-skills/AGENTS.md`

## Security Considerations

Skill files are agent instructions, not passive documentation. A malicious or
compromised contribution to `SKILL.md` or any reference file could embed adversarial
directives that alter agent behavior at runtime (prompt injection). The same applies
to `plugin.json`, which is a trust anchor — it controls which skill directories are
loaded at install time.

Standard pull-request review with human sign-off is the appropriate control. The
impact is bounded: the koto-skills plugin affects only agents that have explicitly
installed it, and the content is fully auditable as plain text. No automated mitigation
is required, but reviewers should read skill file changes with the same scrutiny they'd
apply to code that executes with agent permissions.

No other security dimensions apply: no external artifacts are fetched or executed, no
elevated filesystem or network permissions are required, and no sensitive data is
accessed or embedded in the skill content.

**Hooks policy.** Plugin directories can contain a `hooks.json` that executes
arbitrary shell commands on the agent's machine. The koto-skills plugin must not
include `hooks.json` without explicit review. If hooks are added in the future,
the review bar is higher than for skill content changes — hooks execute code, not
instructions.

## Consequences

### Positive

- Agents running koto-backed workflows have a skill that accurately describes the
  current runtime loop — no re-reading source code on each session.
- koto-author reflects the current gate model. Templates produced by agents
  following it will compile in strict mode.
- The root AGENTS.md loads for every Claude Code session in the koto directory,
  replacing a 550-line file that most sessions never saw.
- Reference file boundaries keep SKILL.md scannable while preserving depth for
  agents that need it.

### Negative

- Three new reference files in koto-user and the extended Layer 3 in koto-author
  become new maintenance surfaces. CLI changes must be reflected in
  command-reference.md; gate output schema changes must be reflected in both
  template-format.md and response-shapes.md.
- koto-author's Layer 3 section grows by roughly 40-60 lines. Not a structural
  problem, but the section becomes the longest in the file.
- The action dispatch table in SKILL.md and the action value schemas in
  response-shapes.md overlap. Keeping them consistent requires discipline.

### Mitigations

- The CLAUDE.md protocol in `CLAUDE.md` (already implemented) requires skill
  assessment on every `src/` or `cmd/` change, covering both broken contracts and
  new surface. This is the primary guard against the maintenance surface risk.
- The annotated YAML examples in template-format.md and the field tables sit
  side-by-side, creating a local consistency check: if field names in the table
  and the example disagree, the error is visible on the same screen.
- The dispatch table in SKILL.md contains only one-liners; the full schemas live
  exclusively in response-shapes.md. The overlap is intentional and bounded.

