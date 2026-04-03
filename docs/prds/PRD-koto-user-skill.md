---
status: Draft
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
  An eval harness catches skill drift before it accumulates.
---

# PRD: koto-user skill and koto-skills plugin update

## Status

Draft

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
- An eval harness runs automatically on plugin changes and advises on source
  changes, so skill drift is visible within the same PR cycle that introduced it.

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

**As a koto maintainer**, I want eval cases that run on source PRs so I see
immediately when a code change makes skill guidance stale, even before a
dedicated skill-update PR is filed.

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
- `context-matches`: `matches` (boolean), `error` (string)

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
koto is, the key CLI commands an agent is likely to encounter, where to find
the koto-author and koto-user skills, and a pointer to `docs/guides/cli-usage.md`.
Length must not exceed 80 lines.

**R12** — Delete `plugins/koto-skills/AGENTS.md`. That file's content (koto next
response shapes, error codes, worked examples) must move into koto-user's
`references/` directory. After migration, `plugins/koto-skills/AGENTS.md` must
be deleted entirely.

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
- The `evidence_required` action's three sub-cases (agent data needed, gate
  failed with accepts block, auto-advance candidate) and how to distinguish them
  via `expects.fields`

**R15** — SKILL.md must not reference phantom commands. `koto status` and `koto
query` must not appear in any koto-user file.

**R16** — Custom `references/` files, not AGENTS.md. Skill-specific reference
content (response schemas, error codes, worked examples) must live in files under
`skills/koto-user/references/`, linked explicitly from SKILL.md. The
`plugins/koto-skills/AGENTS.md` content migrated per R12 becomes these files.

**R17** — `references/` must cover at minimum:
- Runtime command reference: the full koto CLI surface relevant to workflow runners
  (init, next with all flags, cancel, rewind, workflows, session dir, decisions
  record/list, overrides record/list, context add/get/exists/list)
- Response shapes: annotated JSON examples for each `action` value, including
  `expects` schema structure and `blocking_conditions` item schema
- Error handling: exit codes (0/1/2/3 semantics), error categories, and correct
  agent responses for each

### Freshness mechanism

**R18** — Create 8 eval cases in `plugins/koto-skills/evals/`. Each case must
have `prompt.txt`, `skill_path.txt`, and `patterns.txt` per the format `eval.sh`
expects. The 8 cases must target:
1. `koto overrides record` exact syntax (not a generic "record" command)
2. Two-step override flow: record first, then call `koto next` again
3. `agent_actionable: true` as the signal that overrides are possible
4. `evidence_required` action with non-empty `blocking_conditions` (gate failed
   but accepts block present)
5. `koto next --to` skipping gate evaluation
6. `--full` flag to restore `details` on repeat visits
7. `koto workflows` as the correct command to list active workflows (not `koto status`)
8. Reserved `"gates"` key rejected in `--with-data` evidence payloads

**R19** — Expand `eval-plugins.yml` to trigger on `src/**` changes. Failing evals
on source-only PRs must not block merge — they must report as advisory output only.
Failing evals on `plugins/**` changes retain their current behavior.

**R20** — CLAUDE.local.md trigger list maintained. The "koto-skills Plugin
Maintenance" section in `CLAUDE.local.md` must be kept current as koto evolves.
(Already implemented; this requirement ensures it's not removed.)

## Acceptance Criteria

### koto-author update

- [ ] `koto status` does not appear in any koto-author file
- [ ] `koto query` does not appear in any koto-author file
- [ ] `template-format.md` Layer 3 includes `gates.<name>.<field>` syntax with a complete example
- [ ] All three gate type output schemas are documented (command, context-exists, context-matches)
- [ ] `override_default` field is documented with resolution order
- [ ] `koto overrides record` and `koto overrides list` are documented with all flags
- [ ] `blocking_conditions` item schema (including `agent_actionable` and `output`) is documented
- [ ] `--allow-legacy-gates` flag and D5 diagnostic are documented
- [ ] `complex-workflow.md` gates-bearing states use `gates.*` routing; no D5 warnings under strict compile
- [ ] `koto-author.md` compile_validation gate uses `gates.template_exists.exists: true` routing
- [ ] compile_validation directive includes a D5 bullet in the error list

### Root AGENTS.md

- [ ] `AGENTS.md` exists at the koto repo root
- [ ] File length ≤ 80 lines
- [ ] Covers: koto purpose, key CLI commands, pointers to koto-author and koto-user skills, cli-usage.md reference
- [ ] `plugins/koto-skills/AGENTS.md` is deleted

### koto-user skill

- [ ] `plugins/koto-skills/skills/koto-user/SKILL.md` exists
- [ ] `./skills/koto-user` is in `plugin.json` skills array
- [ ] SKILL.md contains the 6-value `action` dispatch table
- [ ] SKILL.md describes the two-step override flow explicitly (record → then call next again)
- [ ] SKILL.md explains the three `evidence_required` sub-cases and how to distinguish them
- [ ] No koto-user file references `koto status` or `koto query`
- [ ] `references/` directory exists with runtime command reference, response shapes, and error handling files
- [ ] Content formerly in `plugins/koto-skills/AGENTS.md` is accessible via koto-user references/

### Freshness mechanism

- [ ] `plugins/koto-skills/evals/` exists with 8 subdirectories, each containing `prompt.txt`, `skill_path.txt`, `patterns.txt`
- [ ] All 8 behaviors from R18 are covered by at least one eval case
- [ ] `eval-plugins.yml` triggers on both `plugins/**` and `src/**` path changes
- [ ] Evals failing on source-only PRs are advisory (do not block merge)
- [ ] CLAUDE.local.md "koto-skills Plugin Maintenance" section is present

## Out of Scope

- New koto gate types (jira, http, json-command)
- Changes to koto CLI behavior or template format (PRD covers documentation, not engine changes)
- Visualization UI
- Updating other plugins or skills outside koto-skills
- Fixing `koto query` in CLAUDE.local.md as a documentation-only task (the fix is removing the reference; CLAUDE.local.md is not a committed file this PRD controls)
- Context-matches gate semantics beyond what current tests demonstrate (requires separate investigation)

## Open Questions

- **`context-matches` gate**: the output schema is documented in source (`matches: bool, error: string`) but no functional test fixture was examined. Should koto-user and koto-author document this gate type fully, or mark it as "available but behavior pending documentation"?

## Known Limitations

- The eval harness uses positive-only pattern matching (`grep -qP`). Evals can confirm a behavior is mentioned but cannot verify it's described correctly. Semantic accuracy beyond keyword presence requires human review.
- Evals on source-only PRs are advisory: a maintainer may not notice or act on advisory failures. The CLAUDE.local.md trigger list remains the primary human signal.
- `--allow-legacy-gates` is transitory — it will be removed once the shirabe `work-on` template migrates to structured gate routing. R7 documentation should note this.

## Decisions and Trade-offs

**Same plugin for koto-user (not separate plugin)**: Adding koto-user to `koto-skills` requires one directory and one `plugin.json` line. A separate plugin would split the install step without benefit — agents who run workflows typically do so within projects already using koto-skills for authoring. The Stop hook and plugin infrastructure benefit both skills without duplication.

**Root AGENTS.md is orientation-level, not a skill substitute**: Three functional substitutes for koto-user already exist (AGENTS.md 550 lines, koto.mdc 207 lines, cli-usage.md). The root AGENTS.md is not replacing them — it's a short entry point that orients any Claude Code session and points to the skill for detailed guidance. Content depth lives in the skill's references/, not in AGENTS.md.

**Advisory evals on source PRs (not blocking)**: Blocking evals on source changes would prevent merging legitimate refactors that don't change observable behavior. Advisory mode keeps the signal visible without adding false-positive friction. The CLAUDE.local.md trigger list handles the human review signal.

**File-change heuristics ruled out**: Heuristic CI checks (require skill file changes when certain source files change) have a high false-positive rate — not every engine refactor changes observable behavior. Advisory evals cover the semantic gap more reliably.

**plugins/koto-skills/AGENTS.md deleted (not kept as pointer)**: After its content
migrates to koto-user/references/, the plugin-root AGENTS.md is deleted outright.
Keeping a pointer would leave an AGENTS.md that Claude Code might auto-load from
ancestor directories, creating confusion about where authoritative content lives.
Content consumers get the reference material through the skill mechanism, which
is the correct channel.

**koto-author updates fit in existing files (no new reference files)**: The 4 high-severity gaps all extend sections that already exist in `template-format.md` and the example files. Creating new reference files would fragment coverage; extending existing Layer 3 and adding to the gate examples preserves the skill's structure.
