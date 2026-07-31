---
complexity: testable
complexity_rationale: Pure refactor of two already-shipped, already-tested construction sites with a real regression risk (wire format, existing test suite) but no new behavior or new call sites -- warrants explicit regression testing but not the deeper cross-cutting scrutiny of a critical change.
---

## Goal

Refactor both existing `SchedulerWarning::StaleTemplateSourceDir` construction sites -- `src/engine/path_resolution.rs`'s per-task resolver and `src/cli/batch.rs::emit_template_source_dir_warnings` -- to build from the shared `check_template_source_path`/`check_template_source_dir` module (<<ISSUE:1>>) instead of computing `Path::exists()`/`current_machine_id()` inline, with zero change to observable scheduler behavior.

## Context

koto's batch scheduler has two independent places that construct `SchedulerWarning::StaleTemplateSourceDir` today, both computing the same underlying fact ("does this recorded `template_source_dir` still exist, and on whose machine") by hand:

- `src/engine/path_resolution.rs`'s `resolve_template_path_with_base_status` (the per-task resolver) builds the warning inline at lines 175-179, calling `current_machine_id()` directly (line 177) against a pre-computed `base_exists: Option<bool>` passed in by the caller.
- `src/cli/batch.rs`'s `emit_template_source_dir_warnings` (line 1774, called once per scheduler tick from line 882) independently computes `template_source_dir_exists` via `Path::exists()` at lines 875-876 in its caller, then builds the warning inline at lines 1789-1793, calling `crate::engine::path_resolution::current_machine_id()` directly (line 1791).

<<ISSUE:1>> introduces `src/engine/template_source_status.rs` with `TemplateSourceStatus { path, exists, machine_id }` and two functions: the core `check_template_source_path(path: Option<&Path>) -> Option<TemplateSourceStatus>` (which `batch.rs` needs, since its call site only has `Option<&Path>` in scope, not a `StateFileHeader`) and the header-accepting wrapper `check_template_source_dir(header: &StateFileHeader) -> Option<TemplateSourceStatus>` (which `path_resolution.rs` can use if/when a header is in scope at its call site). This issue is the second of two consolidation steps the design calls for before three new consumers (`SessionInfo`, `koto status`, `koto init`) get wired up in later issues -- without it, the codebase would keep hand-rolling the same `Path::exists()`/`current_machine_id()` fact in two places even after the shared module exists, undercutting the design's "one computation, not four (now five)" goal.

This is pure infrastructure risk-reduction with a real regression risk attached: it must not change the scheduler's public JSON wire format (`kind`, `path`, `machine_id`, `falling_back_to` on `stale_template_source_dir`), and the existing regression-guard tests for both sites must keep passing unmodified.

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

## Acceptance Criteria

- [ ] `src/engine/path_resolution.rs`'s `StaleTemplateSourceDir` construction (currently inline at lines 175-179, calling `current_machine_id()` directly) is refactored to build from the shared module (<<ISSUE:1>>) instead of computing `current_machine_id()`/existence inline at this call site. `falling_back_to` (a resolver-only fallback decision with no meaning in the shared core) continues to be computed and attached locally, as today.
- [ ] `src/cli/batch.rs::emit_template_source_dir_warnings` (line ~1774) is refactored to build its `StaleTemplateSourceDir` warning from `check_template_source_path` (the core, `Option<&Path>`-accepting function -- this call site has no `StateFileHeader` in scope) instead of accepting a pre-computed `base_exists: Option<bool>` and calling `crate::engine::path_resolution::current_machine_id()` directly (currently line 1791). `falling_back_to` continues to be computed and attached locally from `submitter_cwd`, as today.
- [ ] Neither refactor changes the scheduler's public JSON shape: `SchedulerWarning::StaleTemplateSourceDir`'s `#[serde(tag = "kind", rename_all = "snake_case")]` output (`kind`, `path`, `machine_id` with `skip_serializing_if`, `falling_back_to`) is byte-for-byte identical before and after this change. No fields are added, removed, or renamed on the enum variant itself.
- [ ] `current_machine_id()` in `src/engine/path_resolution.rs` (line 66, currently `pub(crate)`) either stays in place and is called from within the shared module's implementation, or is relocated/re-exported such that both refactored call sites and the shared module resolve to the same single implementation -- no second, divergent copy of machine-id detection is introduced.
- [ ] The full existing test suite for both refactored modules passes unmodified (no test bodies, assertions, or expected values changed) -- specifically, in `src/engine/path_resolution.rs`'s `#[cfg(test)] mod tests`: `stale_base_emits_warning_with_machine_id_and_fallback`, `stale_base_without_cwd_falls_back_to_target_path`, `missing_base_emits_warning_and_falls_back_to_cwd`, `no_base_no_cwd_returns_relative_path_with_missing_warning`, and `current_machine_id_returns_some_or_none_consistently` all continue to pass as-is.
- [ ] `src/engine/scheduler_warning.rs`'s own existing serialization tests (`stale_serializes_with_kind_discriminator`, `stale_omits_machine_id_when_none`, `round_trip`) continue to pass unmodified -- these are the hard-coded wire-format regression guard the design calls out explicitly.
- [ ] `src/cli/batch.rs`'s existing test suite (`cargo test --lib` covering the `cli::batch` module) passes unmodified; note there is no test in `batch.rs` today that names `emit_template_source_dir_warnings` or asserts on its `StaleTemplateSourceDir` output directly by name -- its behavior is exercised indirectly through broader scheduler-tick tests, all of which must keep passing.
- [ ] Full workspace test run (`cargo test`) passes with no test file modified as part of this change -- this issue changes only production code in `path_resolution.rs` and `batch.rs`, plus whatever import/`use` additions are needed to reach the shared module from <<ISSUE:1>>.
- [ ] `cargo build` and `cargo clippy` (or the project's standard lint target) are clean, with no new warnings introduced by the refactor (e.g. unused `base_exists` parameter, now-dead pre-computation code at `batch.rs` lines 875-876 if it becomes redundant, or an unused `current_machine_id` import at either call site).

## Dependencies

Blocked by <<ISSUE:1>>

## Downstream Dependencies

None (leaf node)
