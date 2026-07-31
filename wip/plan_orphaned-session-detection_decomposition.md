---
design_doc: docs/designs/DESIGN-orphaned-session-detection.md
input_type: design
decomposition_strategy: horizontal
strategy_rationale: "The design refactors existing scheduler code and extends an already-working check layer-by-layer (shared core, then both existing consumers, then three new read surfaces); there is no new end-to-end user flow that benefits from an early stub-and-refine skeleton -- each phase is independently complete and testable."
confirmed_by_user: true
issue_count: 5
execution_mode: single-pr
---

# Plan Decomposition: DESIGN-orphaned-session-detection

## Strategy: Horizontal

Issues mirror the design's own five Implementation Approach phases exactly:
a foundational shared module, then both existing scheduler construction
sites refactored to consume it (regression-guarded by existing tests),
then the session-info/backend plumbing, then the two new read-only CLI
surfaces, then the one surface with new I/O (`koto init`'s collision
path) last -- lowest-risk change first, riskiest (new I/O on an existing
hot error path) last.

## Docs-Coverage Emit (step 3.1a)

`user_visible_surface` is absent from the design's frontmatter. The design
body references `docs/guides/cloud-sync-setup.md` in Context and Problem
Statement -- the prose fallback triggers: user-visible surface is
**present**. Docs coverage is folded into Issue 4's acceptance criteria
(documents the new `stale_template_source_dir` field in
`docs/guides/cli-usage.md`), rather than a dedicated docs issue, since the
new user-facing surface (two new JSON fields) is small enough to cover
alongside the issue that introduces it.

## Issue Outlines

### Issue 1: feat(engine): add shared template-source-status check module
- **Type**: standard
- **Complexity**: simple
- **Goal**: Introduce `TemplateSourceStatus` and
  `check_template_source_path`/`check_template_source_dir` in a new
  `src/engine/template_source_status.rs`, unit-tested in isolation, with
  no wiring into any existing call site yet.
- **Section**: Solution Architecture / Implementation Approach Phase 1
- **Milestone**: Orphaned Session Detection
- **Dependencies**: None

### Issue 2: refactor(engine): route stale-template-source-dir warnings through the shared module
- **Type**: standard
- **Complexity**: testable
- **Goal**: Refactor both existing `SchedulerWarning::StaleTemplateSourceDir`
  construction sites (`path_resolution.rs`'s per-task resolver and
  `batch.rs::emit_template_source_dir_warnings`) to build from the Issue 1
  module instead of computing `Path::exists()`/`current_machine_id()`
  inline, with no wire-format change. Existing tests
  (`stale_base_emits_warning_with_machine_id_and_fallback` and neighbors)
  must keep passing unmodified as the regression guard.
- **Section**: Implementation Approach Phase 2
- **Milestone**: Orphaned Session Detection
- **Dependencies**: Issue 1

### Issue 3: feat(session): thread template-source-status through SessionInfo and both list() backends
- **Type**: standard
- **Complexity**: testable
- **Goal**: Add an additive `template_source_status: Option<TemplateSourceStatus>`
  field to `SessionInfo`; populate it in `LocalBackend::list()` from the
  in-memory header (no new I/O); leave it `None` in `CloudBackend::list()`'s
  remote-only placeholder rows and populate normally for rows already
  synced locally. Add a doc comment on the field noting the `CloudBackend`
  None-means-two-things limitation the design accepts.
- **Section**: Implementation Approach Phase 3
- **Milestone**: Orphaned Session Detection
- **Dependencies**: Issue 1

### Issue 4: feat(cli): surface stale template_source_dir on koto status and koto session list
- **Type**: standard
- **Complexity**: testable
- **Goal**: Add a conditional `stale_template_source_dir` JSON key to
  `handle_status`'s response and surface `template_source_status` from
  each row in `handle_list`'s output, via a shared backend-aware wording
  helper (`Backend::is_cloud()`-gated, per Decision 2) that softens
  language for cloud-synced sessions. Document the new field in
  `docs/guides/cli-usage.md` (docs-coverage emit, step 3.1a).
- **Section**: Implementation Approach Phase 4
- **Milestone**: Orphaned Session Detection
- **Dependencies**: Issue 1, Issue 3

### Issue 5: fix(cli): diagnose stale template_source_dir in koto init's already-exists error
- **Type**: standard
- **Complexity**: testable
- **Goal**: Update both `koto init` collision paths (the pre-check and the
  `SpawnErrorKind::Collision` handler) to open the colliding session's
  header, run the Issue 1 check, and append the same staleness clause to
  both -- this is the design's Implicit Decision: the two base messages are
  not byte-identical today and this issue does not unify them, only
  ensures the new clause is present on both. This is the fix for the
  originally reported bug (tsukumogami/koto#189).
- **Section**: Implementation Approach Phase 5
- **Milestone**: Orphaned Session Detection
- **Dependencies**: Issue 1

<!-- decision:start id="orphaned-session-detection-value-confirmation" status="confirmed" -->
### Decision: Value confirmation (step 3.5a)

**Context**: Single-pr mode was explicitly specified by the user
(`--single-pr`), so there is exactly one unit -- the whole plan -- landing
as one PR.

**Assumptions**: None beyond the user's explicit mode choice.

**Chosen**: Pass by construction. A single-PR unit passes the value test
by definition (per phase-3-decomposition.md's Value Confirmation section):
it is not split into multiple PR-shaped units, so there is no
"building-block vs. standalone increment" question to adjudicate. The one
unit lands the full fix described in the design's Consequences/Positive
section (tsukumogami/koto#189's collision-diagnosis bug fixed, passive
list-view staleness visibility, scheduler behavior preserved).

**Rationale**: The surfaced rule on the plan SKILL only requires
per-unit value confirmation when a plan is split across multiple PRs. No
split was requested or proposed here.

**Alternatives Considered**: N/A -- not applicable when there is one unit.

**Consequences**: None; proceeds directly to execution mode finalization
(already settled as single-pr by explicit user instruction).
<!-- decision:end -->
