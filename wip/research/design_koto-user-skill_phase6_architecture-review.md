# Architecture Review: koto-user Skill Design

Date: 2026-04-03
Phase: 6 — Architecture Review
Reviewer: Agent (architecture-review role)

---

## Scope

This review evaluates the proposed solution architecture for the koto-skills plugin update,
covering: koto-author corrections, new koto-user skill, root AGENTS.md migration, and
deletion of `plugins/koto-skills/AGENTS.md`.

Sources consulted:
- `plugins/koto-skills/.claude-plugin/plugin.json`
- `plugins/koto-skills/AGENTS.md` (current)
- `plugins/koto-skills/skills/koto-author/SKILL.md`
- `plugins/koto-skills/skills/koto-author/references/template-format.md`
- `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md`
- `plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md`
- `src/cli/mod.rs` (command enumeration)
- `src/cli/next_types.rs` (response schema)
- `src/template/types.rs` (D5 gate validation)
- `src/template/compile.rs` (--allow-legacy-gates flag)
- `src/engine/advance.rs` (gates.* routing)

---

## 1. Is the architecture clear enough to implement?

**Yes, with one gap.** The file tree, content contracts, and phase sequencing are specific
enough that an implementer can begin without additional design work. The six action values
are enumerated, the three sub-cases for `evidence_required` are noted, and the two-step
override flow is explicit.

The one gap: the design mentions `gates.* routing` and `GateResult fields` as content that
must be added to `template-format.md` Layer 3, but does not spell out what those fields are.
The implementer will need to read `src/gate/` and `src/engine/advance.rs` to know what
`StructuredGateResult` serializes to (e.g., `exit_code`, `stdout`, `stderr` for `command`
type gates). That discovery step is non-trivial and should be documented in the design or
delegated explicitly to a sub-task.

---

## 2. Are there missing components or interfaces?

**Three gaps found, one minor.**

### 2a. Phantom commands in koto-author SKILL.md (confirmed)

`koto status` appears three times in `SKILL.md` (lines 55, 88, 107) and once in the
"session already exists" troubleshooting block. The CLI source (`src/cli/mod.rs`,
`Command` enum) has no `Status` variant — the command does not exist. The correct
substitute is `koto workflows` (to list active sessions) followed by `koto next <name>`
(to get the current directive). The design correctly identifies this as a Phase 1 fix.

### 2b. `koto context` commands are undocumented in the proposed koto-user content

The `complex-workflow.md` example template (used by `koto-author`) references
`koto context add` in its `build` state directive:

> "Compile the application, package it as `build-output.tar.gz`, and submit it to the
> content store with `koto context add`."

`koto context add`, `koto context get`, `koto context exists`, and `koto context list`
are implemented in `src/cli/mod.rs` (lines 707-752). The `context-exists` and
`context-matches` gate types depend on content submitted via `koto context add`. An
agent running a workflow that uses these gate types needs to know the command interface.

The design's `command-reference.md` for koto-user should include `koto context`
subcommands. Without them, an agent encountering a `context-exists` gate blocking
condition won't know how to resolve it.

### 2c. `--allow-legacy-gates` flag missing from koto-author compile_validation directive (confirmed)

The D5 legacy-gate error is real (`src/template/types.rs` lines 535-557, confirmed by
compile tests at `src/template/compile.rs` lines 1099-1144). The `compile_validation`
state in `koto-author.md` lists common compiler errors and fixes but omits D5. The design
correctly flags this as a Phase 1 fix to the template's `compile_validation` directive.

### 2d. `gates.template_exists.exists` gate routing not used in koto-author.md (minor)

The design says to update `compile_validation` to use `gates.template_exists.exists`
routing. Looking at the actual `koto-author.md` template, `compile_validation` already
has a `template_exists` gate (`type: context-exists`), but the transitions use evidence
routing (`compile_result: pass/fail`), not `gates.*` routing. This is valid — the gate
blocks until a template exists, then the agent submits evidence. No `gates.*` when-clause
is needed here because the gate is binary (pass/block). The design's description of this
update is imprecise; the real fix is just ensuring the `compile_validation` directive in
`koto-author.md` tells agents about the D5 error and `--allow-legacy-gates`.

---

## 3. Are the implementation phases correctly sequenced?

**Yes.** The sequencing is sound:

- Phase 1 (koto-author corrections) must precede Phase 2 (koto-user creation) because
  `koto-user/SKILL.md` will reference the corrected koto-author patterns as a structural
  model. If koto-author still contains phantom commands during authoring, the koto-user
  skill risks inheriting the same errors by analogy.

- Phase 2 (koto-user creation) must precede Phase 3 (root AGENTS.md) because the new
  root `AGENTS.md` references both skills by name. Writing it before both skills exist
  risks linking to an incomplete skill.

- Phase 3 (root AGENTS.md + delete plugins/koto-skills/AGENTS.md) as the final step is
  correct. Deleting the old AGENTS.md before the root replacement exists would leave a
  gap for any agent that discovers the plugin directory during the transition.

One sequencing note: `plugin.json` update (adding `"./skills/koto-user"` to the `skills`
array) is assigned to Phase 2. This is correct — the plugin entry should be added only
after the skill directory and SKILL.md are written and compilable. Adding it first would
cause plugin load failures during the authoring window.

---

## 4. Are there simpler alternatives that were overlooked?

**One alternative worth noting, but the proposed approach is justified.**

### Alternative: Keep AGENTS.md in plugins/koto-skills/, remove from repo root

The current `plugins/koto-skills/AGENTS.md` already covers the full koto user interface
(response shapes, error codes, execution loop, command reference). It is thorough and
accurate. The proposal migrates its content into `koto-user/SKILL.md` + three reference
files and creates a new, leaner root `AGENTS.md`.

The alternative — keeping the existing `AGENTS.md` in its current location and not
creating a root file — is simpler in terms of file count. However, it has a real
discoverability problem: Codex, Windsurf, and other AGENTS.md-aware platforms only pick
up the file at the repo root, not in subdirectories. A plugin-local AGENTS.md works only
for agents that explicitly read the plugin directory. The design correctly identifies this
limitation and the proposed split (root prose reference + skill reference files) is the
right tradeoff.

### Alternative: Single reference file instead of three

The design splits koto-user references into `command-reference.md`, `response-shapes.md`,
and `error-handling.md`. A single `reference.md` covering all three topics would reduce
file count and avoid agents needing to open multiple files for a single question. The
three-file split is worth keeping only if each file is likely to be read in isolation
during different workflow phases. Given that an agent typically needs command reference,
response shapes, and error handling simultaneously during the execution loop, a single
well-organized reference file may be preferable. This is a minor concern and not a
blocker; either approach works.

---

## 5. Summary findings

1. **Phantom `koto status` commands** in `koto-author/SKILL.md` are confirmed absent from
   the CLI. Phase 1 must replace these with `koto workflows` + `koto next`. Three
   occurrences at SKILL.md lines 55, 88, and 107.

2. **`koto context` subcommands are missing** from the proposed koto-user reference
   content. Agents running workflows with `context-exists` or `context-matches` gates need
   to know `koto context add` to satisfy blocking conditions. Add a `koto context`
   section to `command-reference.md`.

3. **`gates.*` structured output format is unspecified** in the design. The implementer
   must read `src/gate/` source to discover the `StructuredGateResult` JSON shape before
   writing `template-format.md` Layer 3 extension and `response-shapes.md`. This should
   be called out as a required pre-step in Phase 1.

4. **`gates.template_exists.exists` routing note is imprecise** — the existing
   `compile_validation` state uses evidence routing, not `gates.*` routing, and this is
   correct. The real Phase 1 fix for this state is adding D5 error guidance
   (`--allow-legacy-gates`) to the directive text, not changing the routing pattern.

5. **The three-file split for koto-user references** is a minor design choice worth
   revisiting before implementation. If the files are always read together, a single
   reference file reduces cognitive overhead for agents.

---

## Verdict

The architecture is implementable as described. Two corrections are needed before
implementation begins: (a) clarify the `gates.template_exists.exists` item to be a
directive-text fix rather than a routing change, and (b) add `koto context` subcommands
to the koto-user `command-reference.md` scope. All three phases are correctly sequenced
and can proceed in order in a single PR.
