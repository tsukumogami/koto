---
schema: plan/v1
status: Active
execution_mode: single-pr
upstream: docs/designs/DESIGN-orphaned-session-detection.md
milestone: "Orphaned Session Detection"
issue_count: 5
---

# PLAN: Orphaned Session Detection

## Status

Active

## Scope Summary

Extend koto's existing (but scheduler-only) `template_source_dir` staleness check to `koto init`'s collision path, `koto status`, and `koto session list`, via a new shared `TemplateSourceStatus` core reused by the batch scheduler's existing warning path, with backend-aware wording so cross-machine cloud-sync resume isn't misreported as a dead session. Fixes tsukumogami/koto#189. No new CLI flags; automatic cleanup/gc of orphaned sessions is explicitly out of scope (deferred to a follow-up).

## Decomposition Strategy

**Horizontal.** The design refactors existing code and extends an already-working check layer by layer -- a foundational shared module, then the one existing scheduler consumer that needs it, then session/backend plumbing, then the two new read-only CLI surfaces, then the one surface with new I/O (`koto init`'s collision path) last. There is no new end-to-end user flow that would benefit from an early stub-and-refine walking skeleton; each issue is independently complete and testable on landing.

## Issue Outlines

### Issue 1: feat(engine): add shared template-source-status check module

**Goal**: Add `src/engine/template_source_status.rs` defining `TemplateSourceStatus { path, exists, machine_id }`, the core `check_template_source_path(Option<&Path>)` and header-accepting wrapper `check_template_source_dir(&StateFileHeader)`, plus two additional pieces of foundational shared infrastructure moved here during plan review: a `Backend::is_cloud()` accessor on the session backend enum, and a `format_stale_template_source_note(is_cloud: bool) -> &'static str` wording helper. No wiring into any existing call site.

**Acceptance Criteria**:
- [ ] `TemplateSourceStatus` struct, `check_template_source_path`, and `check_template_source_dir` defined per the design's Key Interfaces, with unit tests covering present-and-existing, present-and-missing, and absent `template_source_dir`, plus a dangling-symlink edge case and a regular-file-instead-of-directory error/invalid-input case.
- [ ] `Backend::is_cloud()` added to the `Backend` enum (`src/session/mod.rs`) -- no such method exists on the enum today, only on the concrete `CloudBackend` struct.
- [ ] `format_stale_template_source_note(is_cloud: bool)` defined here (not in Issue 4), since Issue 5 depends only on this issue and needs the same helper.
- [ ] `cargo build`/`cargo test` pass; nothing outside this new module calls any of the three new items yet.

**Dependencies**: None

**Type**: code
**Files**: `src/engine/template_source_status.rs`, `src/session/mod.rs`

### Issue 2: refactor(engine): route stale-template-source-dir warnings through the shared module

**Goal**: Refactor `batch.rs`'s per-tick existence probe and its `SchedulerWarning::StaleTemplateSourceDir` construction (`emit_template_source_dir_warnings`) to build from Issue 1's shared core instead of computing `Path::exists()`/`current_machine_id()` inline. `path_resolution.rs`'s per-task resolver is explicitly **not** touched -- it only ever consumes a pre-computed boolean from its caller and never independently checked existence, so there is nothing there to consolidate (a plan-review correction: an earlier draft of both the design and this plan incorrectly described `path_resolution.rs` as a second refactor target).

**Acceptance Criteria**:
- [ ] `batch.rs`'s per-tick probe (`~line 875-876`) calls `check_template_source_path` instead of `.map(|p| p.exists())`; the derived `Option<bool>` threaded to `spawn_ready_task`/`spawn_skip_marker_task`/`canonical_paths_tried`/`resolve_template_path_with_base_status` is unchanged in type -- no signature changes to those four.
- [ ] `emit_template_source_dir_warnings` (`~line 1774`) builds its warning from the `Option<TemplateSourceStatus>` computed above instead of calling `current_machine_id()` itself.
- [ ] `src/engine/path_resolution.rs` is **not modified** by this issue.
- [ ] Scheduler's public JSON wire format (`kind`, `path`, `machine_id`, `falling_back_to`) is byte-for-byte unchanged; all existing scheduler tests pass unmodified, including `stale_base_emits_warning_with_machine_id_and_fallback` and neighbors.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/cli/batch.rs`

### Issue 3: feat(session): thread template-source-status through SessionInfo and both list() backends

**Goal**: Add an additive `template_source_status: Option<TemplateSourceStatus>` field to `SessionInfo`, populated by `LocalBackend::list()` from the in-memory header (no new I/O) and left `None` by `CloudBackend::list()`'s remote-only placeholder rows.

**Acceptance Criteria**:
- [ ] `SessionInfo` gains the field using `Option` semantics matching `parent_workflow` (not the `String::new()` sentinel idiom `created_at`/`template_hash` use).
- [ ] Doc comment on the field documents the `CloudBackend` None-means-two-things ambiguity (no `template_source_dir` recorded vs. no header available) as a known, accepted limitation.
- [ ] `LocalBackend::list()` populates the field by calling `check_template_source_dir` on the header already in memory.
- [ ] `CloudBackend::list()`'s remote-only placeholder rows explicitly set the field to `None`; rows synced locally inherit `LocalBackend`'s correct population via delegation.
- [ ] Unit tests confirm all four cases: existing/missing/absent `template_source_dir` on `LocalBackend`, and `None` on a `CloudBackend` remote-only placeholder row.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/session/mod.rs`, `src/session/local.rs`, `src/session/cloud.rs`

### Issue 4: feat(cli): surface stale template_source_dir on koto status and koto session list

**Goal**: Add a conditional `stale_template_source_dir` JSON key to `handle_status`'s response and surface `template_source_status` from each row in `handle_list`'s output, calling Issue 1's `format_stale_template_source_note`/`Backend::is_cloud()` rather than defining a new wording helper here. Documents the new field in `docs/guides/cli-usage.md` (this plan's docs-coverage requirement, triggered by the design's `docs/guides/cloud-sync-setup.md` reference).

**Acceptance Criteria**:
- [ ] Both `handle_status` and `handle_list` call Issue 1's `format_stale_template_source_note(backend.is_cloud())` -- no new formatting helper defined in this issue.
- [ ] `handle_status` adds a conditional `stale_template_source_dir` key (present only when `exists == false`), matching the existing `batch`/`superseded_branches` present-only-when-relevant convention; absent (not `null`) otherwise.
- [ ] `handle_list` surfaces `template_source_status` per row with the same wording applied where `exists == false`.
- [ ] `docs/guides/cli-usage.md` updated: the existing `#### session list` section gets the new field noted; the `koto status` bullet under "Batch surface on existing commands" is extended to describe the new conditional key (no dedicated `### status` section exists today to extend instead).
- [ ] Tests confirm: key omitted for no-`template_source_dir`/existing-directory cases; present with direct wording for stale `LocalBackend` sessions; present with softened wording for stale `CloudBackend` sessions; same distinction in `handle_list`.

**Dependencies**: Blocked by <<ISSUE:1>>, <<ISSUE:3>>

**Type**: code
**Files**: `src/cli/mod.rs`, `src/cli/session.rs`, `docs/guides/cli-usage.md`

### Issue 5: fix(cli): diagnose stale template_source_dir in koto init's already-exists error

**Goal**: Update both `koto init` collision paths (the pre-check at `~line 1682` and the `SpawnErrorKind::Collision` handler at `~line 1707`) to open the colliding session's header, run Issue 1's shared check, and append the same staleness clause to whichever base message each already emits. **This is the fix for tsukumogami/koto#189.**

**Acceptance Criteria**:
- [ ] Both collision paths open the colliding session's header (new I/O for the pre-check, which is a pure `backend.exists(name)` check today) and call `check_template_source_dir`.
- [ ] When the result is stale (`exists: false`), both paths append the same staleness clause -- verified identical between the two for the same underlying condition -- to their respective existing base messages. The two base messages themselves are **not** required to become identical (the pre-check keeps its cleanup guidance, the collision handler keeps its shorter text); this is intentional, not an oversight.
- [ ] Message wording calls Issue 1's `format_stale_template_source_note(backend.is_cloud())` -- no separate accessor or helper defined here.
- [ ] When staleness is not confirmed (`None` or `exists: true`), neither collision path's message changes from its current, pre-this-issue text.
- [ ] The new header read is best-effort: a failed read (e.g. corrupt state file) still surfaces the existing `"already exists"` error without the staleness clause, rather than crashing or changing the error entirely.
- [ ] **Repro test for tsukumogami/koto#189**: init a session with a `template_source_dir`, delete that directory, re-run `koto init` with the same name, and assert the error now includes the staleness clause.
- [ ] `cargo build`/`cargo test` pass; existing message-text tests updated only where staleness is part of the fixture.

**Dependencies**: Blocked by <<ISSUE:1>>

**Type**: code
**Files**: `src/cli/mod.rs`

## Implementation Sequence

Per-outline `**Dependencies**:` declarations above form the graph: Issue 1
has none; Issues 2, 3, and 5 are each blocked by Issue 1 only; Issue 4 is
blocked by both Issue 1 and Issue 3.

**Critical path**: Issue 1 -> Issue 3 -> Issue 4 (length 3).

**Immediate start**: Issue 1 -- every other issue depends on it.

**After Issue 1**: Issues 2, 3, and 5 can proceed in any order or in parallel -- they touch disjoint files (`batch.rs`; `session/mod.rs`+`local.rs`+`cloud.rs`; `cli/mod.rs`'s collision paths).

**After Issue 3**: Issue 4 (its other dependency, Issue 1, is already satisfied).

**Recommended order**: 1, then 3 and 5 (and 2, in any order), then 4 last -- since single-pr mode lands all five in one PR, this order minimizes rework risk by building the riskiest new-I/O surface (Issue 5) and the read-only CLI surface (Issue 4) only after their shared foundations are in place and tested.
