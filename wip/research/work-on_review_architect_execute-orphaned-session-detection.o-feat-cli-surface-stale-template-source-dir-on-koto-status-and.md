# Review: Architect

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Fit with the design structure

`DESIGN-orphaned-session-detection.md`'s Solution Architecture names this issue's exact deliverables:
`handle_status` gains the conditional `stale_template_source_dir` key; `handle_list` surfaces
`template_source_status` per row. The commit implements precisely these two changes and nothing more,
matching the design's component boundaries (`src/cli/mod.rs` / `src/cli/session.rs` as the only touched
non-doc files besides the new test).

## Interface contracts

- `check_template_source_dir(&StateFileHeader) -> Option<TemplateSourceStatus>` and
  `format_stale_template_source_note(is_cloud: bool) -> &'static str` (Issue 1) are consumed exactly as
  published -- no signature changes to either, confirmed by `git diff` showing no changes to
  `src/engine/template_source_status.rs`.
- `Backend::is_cloud(&self) -> bool` (Issue 1) is consumed as-is; no changes to `src/session/mod.rs`.
- `SessionInfo.template_source_status: Option<TemplateSourceStatus>` (Issue 3) is consumed as-is via
  `serde_json::to_value`; no changes to `src/session/mod.rs`, `local.rs`, or `cloud.rs`.

## Dependency direction

`src/cli/mod.rs` and `src/cli/session.rs` (CLI layer) depend on `crate::engine::template_source_status`
(engine layer) and `crate::session::Backend` (session layer) -- both pre-existing, correct directions
(CLI -> engine, CLI -> session). No new dependency edges introduced; no engine or session code depends
back on the CLI layer.

## Signature-change assessment (the one structural change in this commit)

`handle_status`/`handle_list`: `&dyn SessionBackend` -> `&Backend`. This narrows the parameter from a
trait object to the concrete enum. Architecturally this is the correct direction given the constraint
(need `Backend`-specific behavior): it mirrors `handle_resolve`'s existing `&Backend` parameter
(`src/cli/session.rs:525`), so the codebase now has one consistent answer -- "functions that need
backend-concrete behavior take `&Backend`; functions that only need the trait's operations take `&dyn
SessionBackend`" -- rather than two competing patterns. The alternative (widening `SessionBackend` with
an `is_cloud()` default-`false` method) would have blurred that boundary by putting a concrete-enum
concern on every trait implementor, including any future third backend or test double that has no
meaningful answer for "is this cloud."

No other call sites of `handle_status`/`handle_list` exist outside `src/cli/mod.rs`'s command dispatch
(verified via `grep -n "handle_status(\|handle_list("`), so the narrowing is safe and complete.

## Verdict

blocking_count: 0
advisory_count: 0

Matches the design doc's component boundaries exactly, reuses Issue 1/3's published contracts without
modification, and the one structural change (signature narrowing) follows an existing in-repo pattern
rather than inventing a new one.
