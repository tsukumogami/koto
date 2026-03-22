# Instruction Audit: shirabe /work-on Skill

**Date:** 2026-03-22
**Subject:** Identify which agent instructions could be eliminated if koto handled the deterministic steps

---

## Total Instruction Footprint

| File | Lines |
|------|-------|
| SKILL.md (root orchestration) | 127 |
| phase-0-context-injection.md | 48 |
| phase-1-setup.md | 80 |
| phase-2-introspection.md | 101 |
| phase-3-analysis.md (phase dispatch) | 46 |
| phase-3-analysis.md (agent instructions) | 156 |
| phase-4-implementation.md | 132 |
| phase-5-finalization.md | 147 |
| phase-6-pr.md | 86 |
| phase-6-design-diagram-update.md | 72 |
| **Total** | **995** |

---

## Phase-by-Phase Breakdown

### SKILL.md — Root Orchestration (127 lines)

**What the agent does:**
- Parses `$ARGUMENTS` to extract issue number or milestone reference
- Resolves milestones: calls `gh issue list`, filters unblocked issues, picks lowest number
- Reads the resolved issue with `gh issue view`
- Checks for blocking/triage labels against CLAUDE.md label vocabulary
- Detects repo visibility from CLAUDE.md
- Loads content governance skill
- Dispatches to phases 0-6 sequentially
- Detects current phase for resume by inspecting artifact files and git log

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 35 | Deterministic | Argument parsing logic, milestone selection algorithm, blocking-label check, resume detection with exact artifact names and git grep commands, phase dispatch table |
| 20 | Judgment | Triage routing, content governance loading, execution mode flag parsing |
| 72 | Structural | Workflow overview table, phase list, output/begin sections |

**Eliminable if koto takes over:** The entire resume detection block (~15 lines), phase dispatch table (~20 lines in workflow overview + execution section), and milestone selection algorithm (~20 lines) are deterministic sequencing. koto's state machine owns this. These sections exist purely because the agent must track state manually.

---

### Phase 0: Context Injection (48 lines)

**What the agent does:**
- Runs `extract-context.sh <N>` script
- Reads the output file `wip/IMPLEMENTATION_CONTEXT.md`
- Fills in YAML frontmatter TODOs by interpreting extracted design context
- Optionally fetches additional context from related docs, PRs, issues, web

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 10 | Deterministic | Run script, read output file |
| 25 | Judgment | Interpreting extracted context, filling frontmatter TODOs, deciding when additional context gathering is needed |
| 13 | Structural/checklist | Quality checklist, proceed instructions |

**Eliminable if koto takes over:** ~8 lines covering "run this script and read the output file." The judgment work (interpreting the context and filling in the summary) is irreducibly agent work.

---

### Phase 1: Setup (80 lines)

**What the agent does:**
- Re-reads the issue (1.1)
- Creates feature branch with naming convention (1.2)
- Runs test suite to record baseline (1.3)
- Creates `wip/issue_<N>_baseline.md` from a template (1.4)
- Commits with prescribed message format (1.5)

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 40 | Deterministic | git checkout -b, run tests, commit with exact format, baseline file template (schema is fixed) |
| 15 | Judgment | Choosing branch prefix (feature/fix/chore), interpreting test output, documenting pre-existing failures |
| 25 | Structural/checklist | Resume check block, success criteria, next phase pointer |

**Eliminable if koto takes over:** ~35 lines. The branch creation command, test run invocation, baseline file template, and commit format are all fixed procedures. koto could execute these as actions and hand the agent only the structured test result to interpret. The resume check block (7 lines) and next-phase pointer (2 lines) become obsolete when koto owns sequencing.

---

### Phase 2: Introspection (101 lines)

**What the agent does:**
- Runs staleness detection script
- Decides whether to skip or continue based on script output
- Launches a sub-agent with issue-introspection skill
- Routes based on agent's recommendation (Proceed/Clarify/Amend/Re-plan)
- Interacts with user for Clarify/Amend cases
- Commits introspection artifact if created

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 25 | Deterministic | Run staleness script, parse JSON output, commit artifact, launch sub-agent with fixed inputs |
| 40 | Judgment | Routing on recommendation, user interaction for Clarify/Amend, deciding how to update the issue |
| 36 | Structural | Agent prompt template, agent output description, handle-recommendation table, success criteria |

**Eliminable if koto takes over:** ~20 lines covering the script invocation, JSON parsing, skip logic (if `introspection_recommended: false`), and commit step. The agent prompt template (~15 lines) could shrink to a reference if koto pre-constructs the prompt. The routing table (10 lines) becomes a koto guard/transition rule rather than agent instruction.

---

### Phase 3: Analysis — Dispatch (46 lines) + Agent Instructions (156 lines)

**What the agent does (dispatch):**
- Parses issue labels to select full vs. simplified plan template
- Decides whether to pass language skill to sub-agent
- Launches analysis agent with specific inputs
- Commits plan after agent returns

**What the sub-agent does (agent instructions):**
- Reads issue JSON and baseline file
- Reads IMPLEMENTATION_CONTEXT.md if present
- Explores codebase with Glob/Grep/Read
- Designs solution, considers 2+ alternatives
- Writes `wip/issue_<N>_plan.md` using the appropriate template
- Returns 2-3 sentence summary

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 20 | Deterministic | Label parsing for plan type, language skill conditional, commit step, file I/O |
| 130 | Judgment | Codebase exploration, alternative design evaluation, approach selection, plan authoring, testing strategy |
| 52 | Structural | Templates (two variants), success criteria lists |

**Eliminable if koto takes over:** ~20 dispatch lines (label parsing, skill loading, commit), which is modest. The plan template (~50 lines) could be provided to the agent as a pre-populated file stub rather than inline instructions, cutting them from the instruction token count without losing the structure. Net instruction reduction: ~35 lines, but high cognitive-load savings from not having to handle the dispatch logic.

---

### Phase 4: Implementation (132 lines)

**What the agent does:**
- Reads plan and IMPLEMENTATION_CONTEXT.md
- Executes A-B-C-D cycle: write code, validate, functional test, write tests
- Updates plan checkboxes and commits after each logical unit
- Tracks coverage (if project tracks it)
- Performs self-review and requirements cross-reference after all steps
- Optionally launches specialized review agents (security/performance/testing/architecture)

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 20 | Deterministic | Run validation commands, check coverage thresholds, mark plan checkboxes, commit format |
| 85 | Judgment | Code writing, test writing, edge case evaluation, design-intent alignment, review scope decision, agent review dispatch, handling blockers |
| 27 | Structural | Resume check, quality gates, success criteria |

**Eliminable if koto takes over:** ~15 lines (validation command invocation, coverage threshold enforcement, commit format). The implementation cycle instructions are fundamentally judgment work. However, the resume check (10 lines) and quality gates (6 lines) become koto pre-conditions/transitions, eliminating those structural blocks.

---

### Phase 5: Finalization (147 lines)

**What the agent does:**
- Decides whether to generate summary (label-based auto-skip logic)
- Code cleanup: removes debug code, dead imports, addressed TODOs
- Runs final test suite
- Creates `wip/issue_<N>_summary.md` with requirements mapping
- Commits summary
- Deletes `wip/` directory
- Commits cleanup
- Runs test suite again
- Evaluates whether to recommend manual testing (`/try-it`)

**Deterministic vs. judgment:**

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 30 | Deterministic | Run tests, `rm -rf wip/`, commit commands with exact formats, coverage output file removal |
| 50 | Judgment | Auto-skip label logic, code cleanup inspection, requirements mapping authorship, manual testing recommendation |
| 67 | Structural | Summary template (the markdown schema), checklist items, success criteria, auto-skip rules |

**Eliminable if koto takes over:** ~35 lines. The `rm -rf wip/`, both commit steps (commit summary, commit cleanup) with their prescribed formats, the two test-run invocations, and the auto-skip label logic are all deterministic. koto could handle the artifact deletion and commit sequencing automatically, leaving only the judgment work: code cleanup review, requirements mapping, and manual testing recommendation.

---

### Phase 6: Pull Request (86 lines) + Design Diagram Update (72 lines)

**What the agent does (phase-6-pr.md):**
- Rebases on latest main (with conflict handling if needed)
- Calls phase-6-design-diagram-update.md if `Design:` reference exists
- Pushes branch with `-u` or `--force-with-lease`
- Creates PR with conventional commit title and body
- Monitors CI in a polling loop until all checks pass
- Fixes failures by type (test/lint/build/flaky)
- Optionally enables auto-merge

**What the agent does (design-diagram-update):**
- Extracts and validates design doc path from issue body
- Reads design doc, finds Mermaid `:::ready` node for the current issue
- Changes it to `:::done`
- Recalculates downstream blocked→ready status
- Validates modified Mermaid syntax
- Includes changes in commit

**Deterministic vs. judgment:**

PR phase:

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 30 | Deterministic | git rebase, git push, gh pr create, gh pr checks --watch loop, enable auto-merge |
| 35 | Judgment | Conflict resolution, PR body authoring, CI failure diagnosis and fix, deciding when to escalate |
| 21 | Structural | Resume check, success criteria, completion report |

Design diagram update:

| Lines (approx) | Category | Examples |
|----------------|----------|---------|
| 55 | Deterministic | Path validation logic, regex patterns, node status change, downstream recalculation algorithm, Mermaid syntax validation |
| 5 | Judgment | Deciding whether to abort on error vs. warn and continue |
| 12 | Structural | "When to run" section, error handling table |

**Eliminable if koto takes over:** The design diagram update is almost entirely deterministic. koto could execute this as a post-implementation action automatically, eliminating all 72 lines from agent instructions. For phase-6-pr.md, the push/rebase/CI-monitoring loop instructions (~25 lines) are deterministic procedure but require failure-handling judgment that's hard to fully automate. The resume check (7 lines) disappears with koto state management.

---

## Summary Table

| Phase | Total Lines | Deterministic | Judgment | Structural | Eliminable (est.) |
|-------|-------------|---------------|----------|------------|-------------------|
| SKILL.md (root) | 127 | 35 | 20 | 72 | 55 |
| Phase 0: Context Injection | 48 | 10 | 25 | 13 | 8 |
| Phase 1: Setup | 80 | 40 | 15 | 25 | 35 |
| Phase 2: Introspection | 101 | 25 | 40 | 36 | 20 |
| Phase 3: Analysis (dispatch) | 46 | 20 | 0 | 26 | 20 |
| Phase 3: Analysis (agent instr.) | 156 | 5 | 130 | 21 | 35 |
| Phase 4: Implementation | 132 | 20 | 85 | 27 | 16 |
| Phase 5: Finalization | 147 | 30 | 50 | 67 | 35 |
| Phase 6: PR | 86 | 30 | 35 | 21 | 7 |
| Phase 6: Design Diagram Update | 72 | 55 | 5 | 12 | 72 |
| **Total** | **995** | **270** | **405** | **320** | **303** |

**Breakdown:**
- Deterministic instructions: ~270 lines (27%)
- Judgment instructions: ~405 lines (41%)
- Structural (templates, checklists, headers): ~320 lines (32%)

**Estimated shrinkage if koto handles deterministic steps:** ~300 lines eliminated (~30% reduction). Additionally, structural boilerplate tied to state management (resume checks, success criteria for deterministic outcomes, next-phase pointers) would drop significantly — an additional ~120 lines of structural scaffolding exists only because the agent owns sequencing. Combined: ~420 lines (~42% reduction).

---

## Top 5 Highest-Impact Sections to Eliminate

### 1. Phase 6: Design Diagram Update (72 lines, all eliminable)

This is the most complete automation target. The entire procedure — path validation, regex node matching, status propagation, Mermaid syntax check — is algorithmic. There is no interpretation required. koto could run this as a post-finalization action triggered whenever the issue body contains a `Design:` reference. Eliminating it removes 72 lines of dense regex instructions that currently occupy significant agent context, and also removes a category of agent error (incorrect regex application).

### 2. SKILL.md Resume Detection and Phase Dispatch (~55 lines eliminable)

The resume detection block (checking artifact files and git log to infer current phase) exists because the agent must reconstruct koto's job from scratch on every invocation. With koto managing state, the agent never needs to infer where it is — koto's `next` output tells it. Similarly, the phase dispatch table and workflow overview exist to explain sequencing the agent must track manually. These become a koto template definition, not agent instructions.

### 3. Phase 1: Setup — Branch Creation, Test Run, Commit (~35 lines eliminable)

Steps 1.2 (branch creation with naming convention), 1.3 (run test suite), 1.4 (create baseline file from fixed template), and 1.5 (commit with prescribed format) are a fixed script. The branch naming convention (feature/fix/chore), test command, and baseline file schema don't change per-issue. koto could execute these as setup actions and deliver the test result to the agent as structured data, leaving only the interpretation step (documenting pre-existing failures).

### 4. Phase 5: Finalization — Artifact Management and Test Runs (~35 lines eliminable)

The two `rm -rf wip/` + commit sequences, the two final test-run invocations, and the auto-skip label logic are deterministic. koto could manage wip/ artifact lifecycle and run final tests as part of the transition to the PR phase. What remains for the agent is code cleanup inspection (judgment) and requirements mapping (judgment).

### 5. Phase 3: Analysis — Dispatch Logic and Plan Template (~35 lines eliminable)

The label-to-plan-type mapping, language skill conditional loading, and commit step (~20 dispatch lines) are pure branching logic. The two plan templates (~50 structural lines) could be provided as pre-populated file stubs that koto materializes into `wip/issue_<N>_plan.md` at phase entry, rather than inline in agent instructions. The agent reads the stub and fills it in, reducing instruction token overhead without losing the plan structure.

---

## Observations on Structural Lines

320 lines (~32%) are classified as structural — templates, checklists, success criteria, next-phase pointers. These exist for two distinct reasons:

1. **State management scaffolding** (resume checks, next-phase pointers, success criteria for deterministic steps): These disappear entirely when koto owns sequencing. Estimate: ~80 lines.

2. **Output schema documentation** (baseline template, plan templates, summary template, PR body guidance): These remain valuable but could be delivered as file stubs rather than inline instructions, reducing their instruction token footprint to a single "read and complete the stub" directive. Estimate: ~90 lines reducible to ~10.

The remaining ~150 structural lines (quality gates for judgment work, checklists that verify agent reasoning) are genuine agent guidance and should stay.
