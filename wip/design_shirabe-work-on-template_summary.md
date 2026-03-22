# Design Summary: shirabe-work-on-template

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** shirabe's work-on and just-do-it skills are structurally identical
single-session workflows maintained separately. Merging them into a koto-backed
template proves koto's engine on a real workflow and enforces phase structure
without adding surface to koto.
**Constraints:**
- koto surface stays minimal — no new subcommands, no integration runner config required
- Agent handles all external actions (git, PR, CI); koto only enforces phase order
- Must support two entry points: GitHub issue (work-on) and free-form description (just-do-it)
- Skip pattern needed: staleness check must route directly to analysis, bypassing introspection

## Key Findings from Exploration
- Merge cost is low: phases 0-2 differ (GitHub issue vs. free-form input), phases 4-6 are identical
- koto template: ~15 states, mostly auto-advancing, evidence gates at decision points
- No integrations needed: agent-as-integration model works for all external actions
- Open design questions: full state list + evidence schema, skip pattern mechanics,
  shirabe invocation (SessionStart hook, koto init, directive loop), session resume behavior

## Approaches Investigated (Phase 1)

- fine-grained: Maximum enforcement (9 states, evidence at every boundary) but 500+ line
  template with ceremony-heavy evidence fields that don't affect routing. Jury routing
  and iterative implementation don't map cleanly to state transitions.
- coarse-grained: Simple 3-4 checkpoint template but loses enforcement at the transitions
  that matter (staleness detection invisible to koto). Audit trail fragments.
- gate-with-evidence-fallback: ~15 states, command gates auto-advance through mechanically
  verifiable phases, evidence gates at decision points and gate-failure overrides.
  Evidence schema = decision record (meaningful enums + rationale), not {done: true}.

## Phase 2 Decision
**Chosen:** gate-with-evidence-fallback

Rationale: Command gates handle deterministic verification (branch exists, file created,
tests pass, CI green). When a gate fails or the work requires judgment, koto falls back
to an evidence schema capturing the agent's decision. Evidence fields are meaningful enums
(e.g., branch_action: created|reused_existing) plus rationale — permanent decision records
in the event log, not completion confirmations.

## Investigation Findings (Phase 3)

- **gate-mechanics**: Schema + compiler already allow co-locating gates and accepts on
  the same state. Engine hard-stops on gate failure without consulting accepts block.
  Two targeted changes needed: (1) advancement loop falls through to NeedsEvidence when
  gate fails + accepts block present; (2) GateBlocked CLI response carries expects schema
  and agent_actionable: true. Evidence submission path and rationale field already wired.
  Implicit convention (presence of both gates + accepts = fallback) preferred over explicit flag.

- **state-list**: 15 states including 3 terminal. Two paths diverge at entry, converge
  at setup (with post-setup routing via workflow_type evidence) and fully merge at analysis.
  6 states have command gates enabling auto-advancement. 4 states always evidence-gated
  (entry, jury_validation, staleness_check, pr_creation). Critical finding: staleness_check
  can never auto-advance — command gates check exit codes only, not script output content.
  workflow_type evidence from entry must be in scope at setup for routing to work.

- **invocation**: koto init <name> --template <path> initializes workflow. koto next <name>
  is unified directive + advance command. Resume: koto workflows then koto next. No koto
  status/query command (documented but not implemented). --var flag not implemented (needed
  for ISSUE_NUMBER in gate commands). Stop hook exists (not SessionStart). shirabe has
  empty koto-templates/ directory. Template discovery is state-file only, not template files.

## Phase 3 Decision Results (shirabe:design Phase 2-3)

6 structured decisions completed and cross-validated:
1. **Mode routing**: Split topology (setup_issue_backed / setup_free_form), no re-submission
2. **Context injection**: Gate on IMPLEMENTATION_CONTEXT.md artifact existence
3. **Free-form validation**: Two states — pre-research (binary) + post-research (ternary)
4. **Introspection outcomes**: Collapse Clarify/Amend into approach_updated; issue_superseded → done_blocked
5. **Error recovery**: retry/escalate enum variants; koto rewind for done_blocked
6. **Directives**: Concise with resume preambles; wrapper injection for complex phases

Cross-validation finding: introspection gate requires --var (unimplemented); operates as evidence-only until --var ships. Template has 17 states.

## Current Status
**Phase:** Phases 1-3 complete (shirabe:design) — Considered Options and Decision Outcome written; proceeding to Phase 4 (Architecture)
**Last Updated:** 2026-03-22
