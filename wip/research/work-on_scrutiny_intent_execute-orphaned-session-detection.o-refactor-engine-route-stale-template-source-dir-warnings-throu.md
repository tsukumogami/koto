# Scrutiny: Intent

Issue 2 -- refactor(engine): route stale-template-source-dir warnings through the shared module.

## Design doc alignment

`docs/designs/DESIGN-orphaned-session-detection.md`'s Solution Architecture section describes
`template_source_status.rs` as the single place in the codebase that answers "does this recorded
directory exist, and on whose machine" -- with the scheduler's existing construction site as one
of the call sites to be routed through it (the module's own doc comment, read directly from
`src/engine/template_source_status.rs`, states this explicitly: "Later issues in the same plan
route the scheduler's existing construction site and the three new consumers through it"). This
implementation does exactly that for the scheduler's two use points (the per-tick existence probe
and the warning constructor) -- nothing more, nothing less.

## Behavior preservation (the real intent of a "pure refactor" issue)

The issue explicitly frames itself as a refactor with "real regression-risk profile" -- the intent
is zero externally observable behavior change. This was validated three ways:
- The full existing test suite (1000+ tests across unit + integration) passes with no test file
  modifications, including the exact named tests called out in the AC
  (`stale_base_emits_warning_with_machine_id_and_fallback` and neighbors in path_resolution.rs,
  though those are untouched entirely; and the scheduler_warning.rs serialization tests).
- The `StaleTemplateSourceDir` construction logic is line-for-line equivalent: `base_exists ==
  Some(false)` becomes `!status.exists` (identical condition, since `status.exists` is always
  populated when `status` is `Some`); `current_machine_id()` becomes `status.machine_id.clone()`,
  and `check_template_source_path` computes `machine_id: current_machine_id()` internally, so the
  value is identical, just computed once inside the shared module instead of at the call site.
- `MissingTemplateSourceDir` still fires in exactly the same condition (no path recorded).

## Foundation for downstream issues

This issue's scope is narrow (batch.rs only) per the plan's explicit correction that
path_resolution.rs is out of scope. It does not block or complicate Issues 3-5, which route three
other call sites (`koto status`, `koto init`, `koto session list`) through the header-accepting
`check_template_source_dir` wrapper -- a separate function this issue does not touch.

## Verdict

blocking_count: 0
advisory_count: 0

Implementation matches both the literal AC text and the design doc's stated purpose for this
module. No shortcuts that would satisfy the letter of the AC while diverging from intent.
