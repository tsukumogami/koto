---
status: Complete
problem: |
  AI agents running koto-backed workflows have no installable skill guiding them
  through the runtime loop. The existing koto-author skill covers template authoring
  but not workflow execution. When koto capabilities change — as happened across
  PRs #120–#125 — skills are not updated alongside code, so agent guidance drifts
  from actual behavior. Two phantom commands (`koto status`, `koto query`) appear
  in the existing skill and CLAUDE.local.md but don't exist in the CLI, and
  koto-author's gate documentation predates structured gate output entirely.
goals: |
  Agents running koto-backed workflows can find and follow a skill that accurately
  describes the current runtime loop. The koto-author skill reflects the gate-transition
  roadmap. A lightweight repo-wide AGENTS.md orients any Claude Code session in koto.
  A CLAUDE.md protocol ensures skill drift is assessed within the same PR cycle that introduced it.
---

# PRD: koto-user skill and koto-skills plugin update

## Status

In Progress

## Problem Statement

AI agents running koto-backed workflows have no installable skill guiding them
through the runtime loop. The `koto-skills` plugin has one skill (`koto-author`)
that covers template authoring, but nothing equivalent for agents who run
koto-backed workflows — agents who need to call `koto init`, interpret `koto
next` output, submit evidence, handle gate blocking, record overrides, and
rewind states.

When koto's capabilities changed across PRs #120–#125 (structured gate output,
override mechanism, compiler validation, backward compatibility), neither the
existing skill nor any documentation was updated alongside the code. An agent
following `koto-author` today will produce legacy-gate templates that fail
`koto template compile` in strict mode and have no guidance on the override
mechanism when gates block.

Two additional problems surface: two phantom commands (`koto status`, `koto
query`) appear in the current skill and workspace config but do not exist in the
CLI; and the plugin-root `plugins/koto-skills/AGENTS.md` is only loaded by
Claude Code when working inside that plugin directory, not when working at the
repo root.

## Goals

- Agents running koto-backed workflows can install the `koto-skills` plugin and
  follow the `koto-user` skill to complete a full workflow session correctly.
- koto-author accurately reflects the current gate model, override mechanism, and
  compiler behavior.
- Any Claude Code session at the koto repo root starts with basic orientation via
  a root-level `AGENTS.md`.
- A CLAUDE.md protocol ensures skill drift is assessed within the same PR cycle that introduced it.

## User Stories

**As an AI agent running a koto-backed workflow**, I want a skill that explains
how to interpret `koto next` output so I dispatch on `action` correctly — choosing
between submitting evidence, overriding a blocked gate, or waiting — without
re-reading the source code each time.

**As an AI agent writing a koto template**, I want documentation on `gates.*`
routing syntax and structured gate output schemas so I produce templates that
compile cleanly in strict mode and route automatically based on gate results.

**As an AI agent in a koto repo session**, I want a root-level `AGENTS.md` to
orient me before I start reading skill files or invoking `koto` — covering what
koto does and where to find guidance.

**As a koto maintainer**, I want a protocol in CLAUDE.md that prompts skill assessment on every source PR, covering both broken contracts and new surface that should be documented.

## Requirements

### koto-author update (Workstream A)

**R1** — Remove phantom command references. All mentions of `koto status` and
`koto query` in koto-author SKILL.md must be replaced with the correct
alternatives (`koto next` to get current state, `koto workflows` to list active
workflows).

**R2** — Document `gates.*` when-clause routing. `references/template-format.md`
Layer 3 must show the `gates.<name>.<field>` path syntax used in transition `when`
blocks, with at least one complete example showing a gate-routed state.

**R3** — Document structured gate output schemas. For each gate type, document
the output fields available for `gates.*` routing:
- `command`: `exit_code` (number), `error` (string)
- `context-exists`: `exists` (boolean), `error` (string)
- `context-matches`: `matches` (boolean), `error` (string) — provisional: document as
  available with the stated schema; note that functional-test verification is pending
  (see Open Questions)

**R4** — Document `override_default`. `references/template-format.md` must
document `override_default` as an optional per-gate field, its type (must match
the gate's output schema), and the three-tier resolution order (`--with-data` >
`override_default` > built-in default).

**R5** — Document the override CLI. `references/template-format.md` must document
`koto overrides record <name> --gate <gate> --rationale <text>` (with `--with-data`
as optional) and `koto overrides list <name>`, including when a template author
would set `override_default` to support the override flow.

**R6** — Document `blocking_conditions` schema. koto-author SKILL.md must document
the full `blocking_conditions` item schema: `name`, `type`, `status`, `agent_actionable`
(and what it means), `output` (gate-type-specific structured data).

**R7** — Document `--allow-legacy-gates` and D5. `references/template-format.md`
must document the `--allow-legacy-gates` flag, the D5 diagnostic (gate with no
`gates.*` routing), and when legacy behavior is acceptable vs. when structured
routing is required.

**R8** — Update complex-workflow.md example. Both gate-bearing states in
`references/examples/complex-workflow.md` must use `gates.*` when routing
(not legacy pass/block). The updated template must not trigger D5 warnings under
`koto template compile`.

**R9** — Update koto-author.md template. The `compile_validation` gate in
`koto-templates/koto-author.md` must use `gates.template_exists.exists: true`
routing. The compile error list must include a D5 entry.

### Root AGENTS.md (Workstream C)

**R10** — Create `AGENTS.md` at the koto repo root. The file must be picked up
by any Claude Code session working in the koto directory (not buried inside the
plugin folder).

**R11** — Root AGENTS.md is orientation-level only. Content must cover: what
koto is, where to find the koto-author and koto-user skills, and a pointer to
`docs/guides/cli-usage.md`. It must explicitly name these CLI commands: `koto init`,
`koto next`, `koto transition`, `koto overrides record`, `koto rewind`, and
`koto workflows`. Length must not exceed 80 lines.

**R12** — Delete `plugins/koto-skills/AGENTS.md`. That file's content (koto next
response shapes, error codes, worked examples) must be reorganized and distributed
into koto-user's `references/` files per the structure in R17. The original file
need not be preserved verbatim; content should be restructured to fit the references/
format. After migration, `plugins/koto-skills/AGENTS.md` must be deleted entirely.

### koto-user skill (Workstream B)

**R13** — Create `plugins/koto-skills/skills/koto-user/` skill directory. Add
`./skills/koto-user` to the `skills` array in `plugins/koto-skills/.claude-plugin/plugin.json`.

**R14** — SKILL.md covers the complete runtime loop. The koto-user SKILL.md must
include:
- When to use the skill (agent is running, not authoring, a koto-backed workflow)
- Session lifecycle: init → next → (evidence | override | wait) → repeat → done
- The `action` field dispatch table: all 6 values (`evidence_required`,
  `gate_blocked`, `integration`, `integration_unavailable`, `done`, `confirm`)
  with the agent behavior for each
- The two-step override flow: `koto overrides record` then `koto next` (not a
  single combined command)
- The `evidence_required` action's three sub-cases and their distinguishing signals:
  (a) agent data needed: `expects.fields` is non-empty and `blocking_conditions` is
  empty; (b) gate failed with accepts block present: `blocking_conditions` is non-empty
  and `expects.fields` is non-empty; (c) auto-advance candidate: `expects.fields` is
  empty and `blocking_conditions` is empty — engine can advance without agent input

**R15** — SKILL.md must not reference phantom commands. `koto status` and `koto
query` must not appear in any koto-user file.

**R16** — Custom `references/` files, not AGENTS.md. Skill-specific reference
content (response schemas, error codes, worked examples) must live in files under
`skills/koto-user/references/`, linked explicitly from SKILL.md. The
`plugins/koto-skills/AGENTS.md` content migrated per R12 becomes these files.

**R17** — `references/` must contain three files with the following names and content:
- `command-reference.md`: documents every koto CLI subcommand relevant to workflow
  runners — `init`, `next` (with `--with-data`, `--to`, `--full` flags), `cancel`,
  `rewind`, `workflows`, `session dir`, `session list`, `session cleanup`, `decisions
  record`, `decisions list`, `overrides record`, `overrides list`, `context add`,
  `context get`, `context exists`, `context list`. This list is exhaustive; each
  subcommand must be present.
- `response-shapes.md`: annotated JSON examples for each of the 6 `action` values,
  the `expects` schema structure, and the `blocking_conditions` item schema
  (including `agent_actionable` and `output`). Must include at least one JSON example
  per action value.
- `error-handling.md`: exit codes 0, 1, 2, and 3 with their semantics and correct
  agent response for each. Must also document `agent_actionable: false` behavior —
  when a gate is blocked and no override is possible, what the agent should do.

### Freshness mechanism

**R18** — CLAUDE.md skill assessment protocol maintained. The "koto-skills Plugin
Maintenance" section in `CLAUDE.md` must remain present and instruct contributors
to assess both skills after any `src/` or `cmd/` change — checking for broken
contracts (existing skill claims that no longer match the code) and new surface
(added behavior not yet covered by either skill). (Already implemented; this
requirement ensures it's not removed or weakened.)

## Acceptance Criteria

### koto-author update

- [ ] `koto status` does not appear in any koto-author file
- [ ] `koto query` does not appear in any koto-author file
- [ ] `template-format.md` Layer 3 includes `gates.<name>.<field>` syntax with a complete example
- [ ] All three gate type output schemas are documented (command, context-exists, context-matches)
- [ ] `override_default` field is documented with all three resolution tiers explicitly stated: `--with-data` > `override_default` > built-in default
- [ ] `koto overrides record` and `koto overrides list` are documented with all flags
- [ ] `blocking_conditions` item schema (including `agent_actionable` and `output`) is documented
- [ ] `--allow-legacy-gates` flag and D5 diagnostic are documented
- [ ] `complex-workflow.md` gates-bearing states use `gates.*` routing; `koto template compile` (without `--allow-legacy-gates`) exits 0 on the updated file
- [ ] `koto-author.md` compile_validation gate uses `gates.template_exists.exists: true` routing
- [ ] compile_validation directive includes a D5 bullet in the error list

### Root AGENTS.md

- [ ] `AGENTS.md` exists at the koto repo root
- [ ] File length ≤ 80 lines
- [ ] Contains the strings "koto-author" and "koto-user" as skill references
- [ ] Contains `docs/guides/cli-usage.md` as a pointer
- [ ] Explicitly names at least these commands: `koto init`, `koto next`, `koto overrides record`, `koto rewind`, `koto workflows`
- [ ] Does not contain `koto status` or `koto query`
- [ ] `plugins/koto-skills/AGENTS.md` is deleted

### koto-user skill

- [ ] `plugins/koto-skills/skills/koto-user/SKILL.md` exists
- [ ] `./skills/koto-user` is in `plugin.json` skills array
- [ ] SKILL.md contains the 6-value `action` dispatch table: `evidence_required`, `gate_blocked`, `integration`, `integration_unavailable`, `done`, `confirm` all appear
- [ ] SKILL.md contains both `koto overrides record` and a phrase indicating the agent must call `koto next` again after recording
- [ ] SKILL.md contains `expects.fields` alongside descriptions of the three `evidence_required` sub-cases
- [ ] SKILL.md or `references/error-handling.md` documents what to do when `agent_actionable` is `false` (gate blocked, override not possible)
- [ ] SKILL.md or `references/command-reference.md` documents `koto rewind` behavior
- [ ] No koto-user file references `koto status` or `koto query`
- [ ] `references/command-reference.md`, `references/response-shapes.md`, and `references/error-handling.md` all exist
- [ ] `references/command-reference.md` contains entries for all subcommands listed in R17
- [ ] `references/response-shapes.md` contains at least one annotated JSON example per action value
- [ ] `references/error-handling.md` documents exit codes 0, 1, 2, and 3 and the `agent_actionable: false` scenario

### Freshness mechanism

- [ ] CLAUDE.md "koto-skills Plugin Maintenance" section is present and has not been removed
- [ ] The section instructs contributors to check for broken contracts and new surface after `src/` or `cmd/` changes

## Out of Scope

- New koto gate types (jira, http, json-command)
- Changes to koto CLI behavior or template format (PRD covers documentation, not engine changes)
- Visualization UI
- Updating other plugins or skills outside koto-skills
- Fixing `koto query` in CLAUDE.local.md as a documentation-only task (the fix was removing the reference; the committed CLAUDE.md does not contain phantom commands)
- Context-matches gate semantics beyond what current tests demonstrate (requires separate investigation)

## Open Questions

- **`context-matches` gate**: the output schema is documented in source (`matches: bool, error: string`) but no functional test fixture was examined. Should koto-user and koto-author document this gate type fully, or mark it as "available but behavior pending documentation"? (R3 uses the provisional marking as the interim answer.)

- **Variable substitution in directives**: `{{VARIABLE_NAME}}` tokens in `directive` and `details` are substituted by the engine before output. Agents receiving raw `{{...}}` tokens vs. substituted values may be confused. Should koto-user's SKILL.md or `references/response-shapes.md` document this behavior explicitly? Neither koto-author nor koto-user currently covers it.

## Known Limitations

- The CLAUDE.md protocol relies on the contributor (human or agent) actually following it. There is no automated enforcement — a PR that skips the skill assessment will not be blocked.
- `--allow-legacy-gates` is transitory — it will be removed once the shirabe `work-on` template migrates to structured gate routing. R7 documentation should note this.

## Decisions and Trade-offs

**Same plugin for koto-user (not separate plugin)**: Adding koto-user to `koto-skills` requires one directory and one `plugin.json` line. A separate plugin would split the install step without benefit — agents who run workflows typically do so within projects already using koto-skills for authoring. The Stop hook and plugin infrastructure benefit both skills without duplication.

**Root AGENTS.md is orientation-level, not a skill substitute**: Three functional substitutes for koto-user already exist (AGENTS.md 550 lines, koto.mdc 207 lines, cli-usage.md). The root AGENTS.md is not replacing them — it's a short entry point that orients any Claude Code session and points to the skill for detailed guidance. Content depth lives in the skill's references/, not in AGENTS.md.

**CLAUDE.md protocol over automated evals**: LLM-based evals with positive-only pattern matching can confirm that keywords appear but cannot verify correctness, and they fundamentally cannot detect new surface added by a PR. A diff-based assessment prompted by CLAUDE.md — checking both broken contracts and new surface — covers both failure modes more reliably than canned test scenarios.

**plugins/koto-skills/AGENTS.md deleted (not kept as pointer)**: After its content
migrates to koto-user/references/, the plugin-root AGENTS.md is deleted outright.
Keeping a pointer would leave an AGENTS.md that Claude Code might auto-load from
ancestor directories, creating confusion about where authoritative content lives.
Content consumers get the reference material through the skill mechanism, which
is the correct channel.

**koto-author updates fit in existing files (no new reference files)**: The 4 high-severity gaps all extend sections that already exist in `template-format.md` and the example files. Creating new reference files would fragment coverage; extending existing Layer 3 and adding to the gate examples preserves the skill's structure.
