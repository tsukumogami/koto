# Scrutiny: Justification

Issue 2 -- refactor(engine): route stale-template-source-dir warnings through the shared module.

## Deviations from the plan/AC text

One judgment call was made and recorded as a decision at the `analysis` state:
`emit_template_source_dir_warnings` was simplified to take a single
`Option<&TemplateSourceStatus>` parameter instead of preserving the original two-parameter
shape (`template_source_dir: Option<&Path>` + `base_exists: Option<bool>`, now replaced by one
status param). The AC text itself only requires the warning to be *built from* the
`Option<TemplateSourceStatus>` -- it does not mandate a specific parameter list, so this is
within scope, not a deviation from the AC.

**Is the reasoning real, not a shortcut?** `check_template_source_path` returns `None` exactly
when its input path argument is `None` (verified by reading `template_source_status.rs`'s
implementation: `path?.to_path_buf()` short-circuits on `None`). So `status.is_none()` is
logically equivalent to `template_source_dir.is_none()` -- carrying both the raw path and the
status would be redundant, since `status.path` already holds the same value the raw path did.
This is a genuine simplification opportunity, not a corner cut: removing the redundant parameter
reduces the chance of the two arguments drifting out of sync in a future edit (e.g., someone
passing a stale `template_source_dir` alongside a freshly recomputed status).

**No other deviations.** The four "must not change" function signatures were left untouched;
`path_resolution.rs` was not touched; test files were not touched. The implementation matches
the plan (`plan.md` in koto context) exactly as analyzed before writing code.

## Verdict

blocking_count: 0
advisory_count: 0

The one recorded decision is well-justified with a concrete correctness argument (provable
equivalence via `check_template_source_path`'s implementation), not a rationalized shortcut.
