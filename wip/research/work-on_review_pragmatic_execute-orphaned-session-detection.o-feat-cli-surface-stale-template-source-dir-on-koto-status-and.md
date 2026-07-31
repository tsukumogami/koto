# Review: Pragmatic

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Simplicity

- `derive_stale_template_source_dir` (`src/cli/mod.rs`) is a five-line helper (excluding doc comment)
  that delegates entirely to Issue 1's `check_template_source_dir` and `format_stale_template_source_note`
  -- no logic duplicated from those functions, no new abstractions introduced.
- `handle_list`'s change (`src/cli/session.rs`) is ~20 lines: serialize, walk the array once, mutate one
  field conditionally, print. No new struct, no new module, no new public API surface beyond the
  signature change already required to reach `backend.is_cloud()`.
- The signature change (`&dyn SessionBackend` -> `&Backend`) touches exactly the two functions that need
  `is_cloud()` and their two call sites, both of which already held a `Backend` value locally -- no
  ripple into other functions that still only need the trait's methods.

## Over-engineering check

- No new trait, no new generic parameter, no new error type. The implementation stays at the same
  abstraction level as the code immediately around it (`derive_batch_view`/`derive_superseded_branches`
  in the same function).
- Considered-and-rejected alternative (adding `is_cloud` to the `SessionBackend` trait) would have been
  more invasive across every backend implementor for no benefit beyond avoiding a two-site signature
  change -- correctly avoided.

## Dead code / scope creep check

- No unused code introduced (`cargo clippy --lib -- -D warnings` clean, which catches unused code among
  other things).
- Strictly scoped to `handle_status`, `handle_list`, and docs -- does not touch `koto init`'s collision
  paths (Issue 5's territory), `batch.rs` (Issue 2, already landed), or `TemplateSourceStatus`'s struct
  definition (Issue 1, already landed and frozen).
- The new integration test file is proportionate to the surface it covers (six focused tests, no
  speculative coverage of unrelated paths).

## Verdict

blocking_count: 0
advisory_count: 0

Minimal, focused diff that reuses existing primitives and existing conventions in the same file rather
than introducing anything new to maintain.
