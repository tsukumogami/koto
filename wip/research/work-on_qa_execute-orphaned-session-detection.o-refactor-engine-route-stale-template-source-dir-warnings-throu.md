# QA Validation

Issue 2 -- refactor(engine): route stale-template-source-dir warnings through the shared module.
Commit: a8944a3.

## Method

`emit_template_source_dir_warnings` has no dedicated unit test in `batch.rs`'s existing test
suite -- the `StaleTemplateSourceDir`/`MissingTemplateSourceDir` construction itself is
well-covered at the type level in `scheduler_warning.rs` (serialization round-trips) and
`path_resolution.rs` (the resolver's own equivalent logic), but this issue's specific call site
(the per-tick emission) was not previously exercised by a named test.

To validate the refactor is behavior-preserving beyond "the full suite passes with no
regressions," two scratch tests were temporarily added to `batch.rs`'s test module (not
committed -- reverted after verification, confirmed via `git diff` showing zero diff on
`batch.rs` afterward):

1. **Stale scenario**: `check_template_source_path` against a known-nonexistent path, then
   `emit_template_source_dir_warnings(Some(&status), Some(&cwd), &mut warnings)`. Asserted the
   single emitted warning is `StaleTemplateSourceDir` with `path` equal to the missing path's
   string form, `machine_id` equal to `current_machine_id()` (the pre-refactor computation,
   proving the value is unchanged), and `falling_back_to` equal to the supplied `submitter_cwd`.
2. **Missing scenario**: `emit_template_source_dir_warnings(None, None, &mut warnings)` --
   asserted the single emitted warning is `MissingTemplateSourceDir`.

## Results

| Scenario | Result |
|---|---|
| Stale template_source_dir -> StaleTemplateSourceDir with correct path/machine_id/fallback | passed |
| No template_source_dir -> MissingTemplateSourceDir | passed |

Both scratch tests passed on the first run (`cargo test --lib qa_scratch` -> 2 passed, 0
failed). They were then removed; `git diff src/cli/batch.rs` after removal is empty, confirming
no leftover artifacts from the QA process.

## Also re-verified

- Full `cargo test`: all tests pass, zero failures.
- `cargo test --lib path_resolution`: 15/15 pass (all named tests from the AC present and
  passing, unmodified).
- `cargo test --lib scheduler_warning`: 5/5 pass, unmodified.
- `cargo clippy --lib -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `git diff --name-only` against the base: only `src/cli/batch.rs` changed.

scenarios_run: 2
scenarios_passed: 2
scenarios_failed: 0
