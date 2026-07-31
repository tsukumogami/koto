---
complexity: testable
complexity_rationale: Refactor of one already-shipped, already-tested construction site (batch.rs) with a real regression risk (wire format, existing test suite) but no new behavior or new call sites -- warrants explicit regression testing but not the deeper cross-cutting scrutiny of a critical change.
---

## Goal

Refactor the batch scheduler's per-tick existence probe and its `SchedulerWarning::StaleTemplateSourceDir` construction in `src/cli/batch.rs` to build from the shared `check_template_source_path` module (<<ISSUE:1>>) instead of computing `Path::exists()`/`current_machine_id()` inline, with zero change to observable scheduler behavior.

## Context

**Correction from plan review (Category B):** an earlier version of this issue also claimed `src/engine/path_resolution.rs`'s per-task resolver (`resolve_template_path_with_base_status`) as a *second* independent `StaleTemplateSourceDir` construction site this issue refactors. That claim doesn't survive contact with the actual call graph: `resolve_template_path_with_base_status` takes `base_exists: Option<bool>` as a parameter from its callers (`spawn_ready_task`, `spawn_skip_marker_task` in `batch.rs`) -- it never independently checked existence, because the one real filesystem probe already happens upstream, once per tick, in `batch.rs` (see below). `path_resolution.rs` is **not modified by this issue** (or this plan at all). Its inline warning construction (lines 175-179, calling `current_machine_id()` itself) is left exactly as it is today -- this is a deliberate scope boundary: threading a full `TemplateSourceStatus` down through `resolve_template_path_with_base_status`'s multiple call sites, purely to avoid one extra cheap, non-filesystem `current_machine_id()` read per task, is not worth the touched-surface-area increase across `spawn_ready_task`/`spawn_skip_marker_task`/`canonical_paths_tried`.

What this issue actually refactors, both in `src/cli/batch.rs`:

- The scheduler's single existing existence probe, currently `template_source_dir.as_deref().map(|p| p.exists())` at `~line 875-876` (assigned to `template_source_dir_exists`), run once per tick before any task is processed.
- `emit_template_source_dir_warnings` (`~line 1774`, called once per tick from `~line 882`), which currently takes a bare `base_exists: Option<bool>` and independently calls `crate::engine::path_resolution::current_machine_id()` (`~line 1791`) to build its `StaleTemplateSourceDir` warning.

<<ISSUE:1>> introduces `src/engine/template_source_status.rs` with `TemplateSourceStatus { path, exists, machine_id }` and the core `check_template_source_path(path: Option<&Path>) -> Option<TemplateSourceStatus>` -- exactly the shape `batch.rs`'s probe needs, since its call site only has `Option<&Path>` in scope, not a `StateFileHeader`.

This is pure infrastructure risk-reduction with a real regression risk attached: it must not change the scheduler's public JSON wire format (`kind`, `path`, `machine_id`, `falling_back_to` on `stale_template_source_dir`), and the existing regression-guard tests must keep passing unmodified. It also must not change the *type* of `template_source_dir_exists` as threaded to `spawn_ready_task`, `spawn_skip_marker_task`, `canonical_paths_tried`, and `resolve_template_path_with_base_status` -- those signatures are unaffected by this issue.

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

## Acceptance Criteria

- [ ] `src/cli/batch.rs`'s per-tick existence probe (`~line 875-876`) is refactored to call `check_template_source_path(template_source_dir.as_deref())` (from <<ISSUE:1>>) instead of `template_source_dir.as_deref().map(|p| p.exists())`, producing an `Option<TemplateSourceStatus>`.
- [ ] The existing `template_source_dir_exists: Option<bool>` value, threaded unchanged to `spawn_ready_task`, `spawn_skip_marker_task`, `canonical_paths_tried`, and (via those) `resolve_template_path_with_base_status`, is derived from the new `Option<TemplateSourceStatus>` via `.map(|s| s.exists)` -- no signature of any of those four functions/call sites changes.
- [ ] `src/cli/batch.rs::emit_template_source_dir_warnings` (`~line 1774`) is refactored to accept the `Option<TemplateSourceStatus>` computed above (instead of a bare `base_exists: Option<bool>`) and builds its `StaleTemplateSourceDir` warning directly from its `.exists` and `.machine_id` fields, rather than calling `crate::engine::path_resolution::current_machine_id()` itself. `falling_back_to` continues to be computed and attached locally from `submitter_cwd`, as today -- it has no equivalent in `TemplateSourceStatus` and is not part of the shared module by design (a scheduler-only fallback decision, not a filesystem fact).
- [ ] `src/engine/path_resolution.rs` is **not modified** by this issue -- confirm via `git diff` that the PR's changes to this issue touch only `src/cli/batch.rs` (plus whatever `use` import is needed to reach `src/engine/template_source_status.rs`).
- [ ] The scheduler's public JSON shape is unchanged: `SchedulerWarning::StaleTemplateSourceDir`'s `#[serde(tag = "kind", rename_all = "snake_case")]` output (`kind`, `path`, `machine_id` with `skip_serializing_if`, `falling_back_to`) is byte-for-byte identical before and after this change. No fields are added, removed, or renamed on the enum variant itself.
- [ ] The full existing test suite for `src/engine/path_resolution.rs` passes unmodified (no test bodies, assertions, or expected values changed) -- specifically `stale_base_emits_warning_with_machine_id_and_fallback`, `stale_base_without_cwd_falls_back_to_target_path`, `missing_base_emits_warning_and_falls_back_to_cwd`, `no_base_no_cwd_returns_relative_path_with_missing_warning`, and `current_machine_id_returns_some_or_none_consistently` all continue to pass as-is -- this issue makes zero changes to that file.
- [ ] `src/engine/scheduler_warning.rs`'s own existing serialization tests (`stale_serializes_with_kind_discriminator`, `stale_omits_machine_id_when_none`, `round_trip`) continue to pass unmodified -- these are the hard-coded wire-format regression guard the design calls out explicitly.
- [ ] `src/cli/batch.rs`'s existing test suite (`cargo test --lib` covering the `cli::batch` module) passes unmodified; note there is no test in `batch.rs` today that names `emit_template_source_dir_warnings` or asserts on its `StaleTemplateSourceDir` output directly by name -- its behavior is exercised indirectly through broader scheduler-tick tests, all of which must keep passing.
- [ ] Full workspace test run (`cargo test`) passes with no test file modified as part of this change -- this issue changes only production code in `batch.rs`, plus whatever import addition is needed to reach the shared module from <<ISSUE:1>>.
- [ ] `cargo build` and `cargo clippy` (or the project's standard lint target) are clean, with no new warnings introduced by the refactor (e.g. an unused `current_machine_id` import at the `emit_template_source_dir_warnings` call site).

## Dependencies

Blocked by <<ISSUE:1>>

## Downstream Dependencies

None (leaf node)
