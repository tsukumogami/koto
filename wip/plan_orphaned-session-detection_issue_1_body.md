---
complexity: simple
complexity_rationale: New, isolated module with unit tests only -- no wiring into any existing call site, no behavior change to code that currently runs.
---

## Goal

Add a new `src/engine/template_source_status.rs` module defining `TemplateSourceStatus` and the `check_template_source_path`/`check_template_source_dir` functions that will become the single shared computation for "does this session's recorded `template_source_dir` still exist, and on whose machine."

## Context

koto's state-file header already records `template_source_dir` (`StateFileHeader.template_source_dir: Option<PathBuf>`, `src/engine/types.rs:260`), but only one existing consumer -- the batch scheduler's per-task path resolver -- reads it back to check staleness, via `SchedulerWarning::StaleTemplateSourceDir` (`src/engine/scheduler_warning.rs`). Three other call sites (`koto init`'s collision check, `koto status`, `koto session list`) need the same fact but have no shared way to compute it today.

This issue extracts that computation into its own module, ahead of any wiring changes, so the four downstream call sites (two existing scheduler construction sites plus three new consumers) can all build on one shared core instead of five independent implementations of the same `Path::exists()` + `current_machine_id()` check. This module introduces no behavior change: it is additive, unit-tested in isolation, and not yet called from any existing code path.

Design: `docs/designs/DESIGN-orphaned-session-detection.md`

## Acceptance Criteria

- [ ] `src/engine/template_source_status.rs` is created and registered as `pub mod template_source_status;` in `src/engine/mod.rs`.
- [ ] Defines a public struct:
  ```rust
  pub struct TemplateSourceStatus {
      pub path: PathBuf,
      pub exists: bool,
      pub machine_id: Option<String>,
  }
  ```
  with a doc comment explaining its purpose (the single shared answer to "does this recorded source directory still exist, and on whose machine") and noting it deliberately excludes any scheduler-only fallback concept (e.g. `falling_back_to`), which stays scoped to `SchedulerWarning::StaleTemplateSourceDir`.
- [ ] Defines the core function:
  ```rust
  pub fn check_template_source_path(path: Option<&Path>) -> Option<TemplateSourceStatus>
  ```
  Returns `None` when `path` is `None`. When `Some`, returns `Some(TemplateSourceStatus { path, exists, machine_id })` where `exists = path.exists()` and `machine_id` comes from `current_machine_id()` (`pub(crate)`, `src/engine/path_resolution.rs:66`) -- called exactly once per invocation.
- [ ] Defines the header-accepting wrapper:
  ```rust
  pub fn check_template_source_dir(header: &StateFileHeader) -> Option<TemplateSourceStatus>
  ```
  Implemented by delegating to `check_template_source_path(header.template_source_dir.as_deref())` (`StateFileHeader` is `src/engine/types.rs:223`, its `template_source_dir` field is `Option<PathBuf>` at line 260).
- [ ] `current_machine_id()`'s visibility in `src/engine/path_resolution.rs` remains `pub(crate)` (or is widened only as far as needed for `template_source_status.rs` to call it from within the `engine` module tree) -- no unrelated visibility changes.
- [ ] Unit tests (in a `#[cfg(test)] mod tests` block within the new file) cover:
  - [ ] Present path that exists on disk (e.g. a `tempfile`/`TempDir`-backed directory): returns `Some` with `exists: true` and the correct `path`.
  - [ ] Present path that does not exist on disk: returns `Some` with `exists: false` and the correct `path`.
  - [ ] Absent path (`None` input to `check_template_source_path`, or a `StateFileHeader` with `template_source_dir: None` for `check_template_source_dir`): returns `None`.
  - [ ] `check_template_source_dir` correctly extracts and delegates using a constructed `StateFileHeader` (existing test helpers/fixtures in `src/engine/types.rs`'s own test module construct `StateFileHeader` values already -- follow that pattern, e.g. `template_source_dir: None` at `src/engine/types.rs:1362`/`1392`/`1502`).
- [ ] `cargo build` and `cargo test` pass with the new module compiled in; no existing test anywhere is modified (this module is not yet called from any existing code path).
- [ ] No other file in the repo calls `check_template_source_path` or `check_template_source_dir` yet -- wiring is explicitly out of scope for this issue (see Downstream Dependencies).

## Dependencies

None

## Downstream Dependencies

- <<ISSUE:2>> (`refactor(engine): route stale-template-source-dir warnings through the shared module`) needs both `check_template_source_path` (core, `Option<&Path>` signature) and `check_template_source_dir` (header-accepting wrapper) to exist and be importable from `src/engine/path_resolution.rs` and `src/cli/batch.rs`, so the two existing `StaleTemplateSourceDir` construction sites can build from this shared core instead of computing `Path::exists()`/`current_machine_id()` inline.
- <<ISSUE:3>> (`feat(session): thread template-source-status through SessionInfo and both list() backends`) needs the `TemplateSourceStatus` struct and `check_template_source_dir` wrapper to be usable from `src/session/local.rs`/`src/session/cloud.rs`, so `LocalBackend::list()` can populate a new `SessionInfo.template_source_status` field from the header it already holds in memory.
- <<ISSUE:4>> (`feat(cli): surface stale template_source_dir on koto status and koto session list`) needs `TemplateSourceStatus`'s fields (`path`, `exists`, `machine_id`) to be `pub` and stable, since they will be projected into JSON output on `koto status`'s conditional `stale_template_source_dir` key and `koto session list`'s per-session output.
- <<ISSUE:5>> (`fix(cli): diagnose stale template_source_dir in koto init's already-exists error`) needs `check_template_source_dir` to be callable from `src/cli/mod.rs` on a freshly-read `StateFileHeader` for the colliding session, so both `koto init` collision paths can append a staleness clause built from this shared helper.
