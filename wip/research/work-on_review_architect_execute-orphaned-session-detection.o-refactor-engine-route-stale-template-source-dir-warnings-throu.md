# Review: Architect

## Design structure fit

`src/cli/batch.rs` (a `cli` module) already depends on `crate::engine::path_resolution` for
`resolve_template_path_with_base_status`. Adding a dependency on
`crate::engine::template_source_status` follows the same existing dependency direction
(cli -> engine), so no new architectural coupling is introduced.

## Interface contracts

- `check_template_source_path`'s contract (`None` in -> `None` out; `Some(path)` in ->
  `Some(TemplateSourceStatus)` out) is used exactly as documented in
  `template_source_status.rs`'s doc comments -- this call site is explicitly named in that
  module's own doc comment as the intended consumer ("used directly by the batch scheduler's
  per-tick probe").
- `emit_template_source_dir_warnings`'s new signature is private (`fn`, not `pub fn`) and only
  has one call site (verified via grep), so narrowing its parameter list has no ripple effect
  elsewhere in the crate.
- The four functions the AC forbids touching (`spawn_ready_task`, `spawn_skip_marker_task`,
  `canonical_paths_tried`, `resolve_template_path_with_base_status`) still take `Option<&Path>`
  + `Option<bool>` exactly as before -- their contract with `path_resolution.rs` is unchanged,
  so Issue 3-5's work (which also touches `template_source_status.rs` consumers) is unaffected
  by this issue.

## Dependency direction

No inversions: `engine::template_source_status` has no dependency back on `cli::batch`, so this
remains a clean one-way dependency from cli down to engine.

## Verdict

blocking_count: 0
advisory_count: 0
