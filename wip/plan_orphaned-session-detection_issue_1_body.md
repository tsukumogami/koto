---
complexity: simple
complexity_rationale: New, isolated module plus a one-line enum accessor, both with unit tests only -- no wiring into any existing call site, no behavior change to code that currently runs.
---

## Goal

Add a new `src/engine/template_source_status.rs` module defining `TemplateSourceStatus` and the `check_template_source_path`/`check_template_source_dir` functions that will become the single shared computation for "does this session's recorded `template_source_dir` still exist, and on whose machine," plus a thin `Backend::is_cloud()` accessor on the session backend enum that later issues need for backend-aware wording.

**Correction from plan review (Category B/D):** an earlier version of this plan left the `Backend::is_cloud()` accessor to be added independently by Issues 4 and 5, creating both a design-vs-plan naming contradiction (the design initially said no new accessor was needed, then reversed that) and a missing-dependency-edge risk (Issue 5 could land before Issue 4 without the accessor existing). This issue now owns the one-line accessor as foundational shared infrastructure, since both Issue 4 and Issue 5 already declare `Blocked by <<ISSUE:1>>` -- resolving both problems without adding a new dependency edge.

## Context

koto's state-file header already records `template_source_dir` (`StateFileHeader.template_source_dir: Option<PathBuf>`, `src/engine/types.rs:260`), but only one existing consumer -- the batch scheduler's per-task path resolver -- reads it back to check staleness, via `SchedulerWarning::StaleTemplateSourceDir` (`src/engine/scheduler_warning.rs`). Three other call sites (`koto init`'s collision check, `koto status`, `koto session list`) need the same fact but have no shared way to compute it today.

This issue extracts that computation into its own module, ahead of any wiring changes, so the downstream call sites (the batch scheduler's per-tick probe, plus three new consumers) can all build on one shared core instead of independently reimplementing the same `Path::exists()` + `current_machine_id()` check. The batch scheduler's *other* existing warning-construction site, `path_resolution.rs`'s per-task resolver, is explicitly out of scope for this whole plan -- it only ever consumes a pre-computed boolean from its caller and never independently checked existence, so there's nothing there for it to consolidate onto. This module introduces no behavior change: it is additive, unit-tested in isolation, and not yet called from any existing code path.

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
  - [ ] At least one edge case beyond plain existence/absence -- a dangling (broken) symlink at the recorded path -- asserting the function's actual, documented behavior: `Path::exists()` follows symlinks and reports `false` for a broken one, so this must return `Some(TemplateSourceStatus { exists: false, .. })`, not `None` and not `exists: true`.
  - [ ] At least one genuine error/invalid-input case: a `template_source_dir` that resolves to a regular file rather than a directory (not missing, not a valid directory) -- assert the function does not error/panic and reports whatever its documented contract says for this case (this function checks path existence only, not "is a directory," so `exists: true` is the correct, asserted outcome here -- the test's job is to confirm the function doesn't silently misreport or fail on a malformed, non-directory `template_source_dir`).
- [ ] `Backend` (`src/session/mod.rs`, `pub enum Backend { Local(LocalBackend), Cloud(CloudBackend) }`) gains:
  ```rust
  pub fn is_cloud(&self) -> bool {
      matches!(self, Backend::Cloud(_))
  }
  ```
  (or an equivalent form delegating to the existing `CloudBackend::is_cloud()` at `cloud.rs:559-561` for the `Cloud` arm). No such method exists on the enum today -- only on the concrete `CloudBackend` struct. Unit test: a `Backend::Local(_)` value returns `false`; a `Backend::Cloud(_)` value returns `true`.
- [ ] `src/engine/template_source_status.rs` also defines the shared wording-formatting helper both later CLI surfaces need:
  ```rust
  pub fn format_stale_template_source_note(is_cloud: bool) -> &'static str {
      if is_cloud {
          "template source directory not found (if this session was synced from another machine, this may be expected)"
      } else {
          "template source directory no longer exists"
      }
  }
  ```
  (exact wording may differ slightly; the shape -- one function, one `bool` parameter, two static strings -- is what matters). Placed here rather than in <<ISSUE:4>> because <<ISSUE:5>> also needs it and depends only on this issue, not on <<ISSUE:4>>: defining it once, here, means neither downstream issue has to hedge on implementation order or risk two independently-written wording strings drifting apart. Unit test: `true` returns the softened/cloud wording, `false` returns the direct/local wording.
- [ ] `cargo build` and `cargo test` pass with the new module, accessor, and helper compiled in; no existing test anywhere is modified (none of the three is yet called from any existing code path).
- [ ] No other file in the repo calls `check_template_source_path`, `check_template_source_dir`, `Backend::is_cloud()`, or `format_stale_template_source_note` yet -- wiring is explicitly out of scope for this issue (see Downstream Dependencies).

## Dependencies

None

## Downstream Dependencies

- <<ISSUE:2>> (`refactor(engine): route stale-template-source-dir warnings through the shared module`) needs the core `check_template_source_path` (`Option<&Path>` signature) to exist and be importable from `src/cli/batch.rs`, so `batch.rs`'s per-tick probe and `emit_template_source_dir_warnings` can build from this shared core instead of computing `Path::exists()`/`current_machine_id()` inline. (`path_resolution.rs` is out of scope for Issue 2 -- it never independently computed existence, only consumed a pre-computed boolean from its caller; see Issue 2's own Context for the corrected scope.)
- <<ISSUE:3>> (`feat(session): thread template-source-status through SessionInfo and both list() backends`) needs the `TemplateSourceStatus` struct and `check_template_source_dir` wrapper to be usable from `src/session/local.rs`/`src/session/cloud.rs`, so `LocalBackend::list()` can populate a new `SessionInfo.template_source_status` field from the header it already holds in memory.
- <<ISSUE:4>> (`feat(cli): surface stale template_source_dir on koto status and koto session list`) needs `TemplateSourceStatus`'s fields (`path`, `exists`, `machine_id`) to be `pub` and stable, AND `Backend::is_cloud()` and `format_stale_template_source_note` to already exist so it consumes them rather than defining its own.
- <<ISSUE:5>> (`fix(cli): diagnose stale template_source_dir in koto init's already-exists error`) needs `check_template_source_dir`, `Backend::is_cloud()`, AND `format_stale_template_source_note` to already exist -- Issue 5 depends only on <<ISSUE:1>>, not <<ISSUE:4>>, so both the accessor and the wording helper must live here (not in Issue 4) to avoid a missing-dependency risk if Issue 5 lands before Issue 4.
