# Review: Architect

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Fit with design structure

This is Phase 5 of the design ("koto init collision messaging"), the last
consumer of Issue 1's shared `template_source_status` module. Dependency
direction is correct and matches the design's Data Flow section: `cli/mod.rs`
depends on `engine::template_source_status` (a lower layer), never the
reverse -- `stale_template_source_dir_clause` is the only new code in this
commit and it only imports from `crate::engine::template_source_status`,
consistent with `handle_status`'s (Issue 4) existing import shape in the
same file.

`Backend` vs. `&dyn SessionBackend`: `handle_init`'s signature change from
`&dyn SessionBackend` to `&Backend` is architecturally sound and precedented
-- `Backend` already implements `SessionBackend` (checked
`src/session/mod.rs:435`), so every existing call inside `handle_init` that
needs the trait's methods (`session::handle_update`, `record_default_intent`,
both declared as `&dyn SessionBackend`) still compiles via Rust's automatic
unsized coercion at the call site; nothing downstream needed to change. This
is the same trade Issue 4 made for `handle_status` (see that function's own
doc comment: "Takes `&Backend`... so it can call the inherent
`Backend::is_cloud()` accessor"). Two now-consistent precedents beat one
special case.

Interface contract of `stale_template_source_dir_clause`: takes `&Backend`
(not `&dyn SessionBackend`) since it needs `is_cloud()`, which is only on the
concrete enum, matching the codebase's existing convention (see
`derive_stale_template_source_dir`, `derive_batch_view` in the same file for
the same shape). Return type `Option<String>` is the right level of
abstraction for this call site specifically -- unlike `handle_status`'s
`derive_stale_template_source_dir`, which returns a `serde_json::Value`
because its caller assembles a JSON response object, `koto init`'s callers
assemble a plain string message, so a bare `Option<String>` clause is the
correct shape for its consumer, not a needless divergence from Issue 4's
return-type choice.

## One structural question (advisory, not blocking)

`stale_template_source_dir_clause` is defined directly above `handle_init`
in `src/cli/mod.rs`, a ~5200-line file that already holds every CLI command
handler plus their private helpers (`derive_stale_template_source_dir`,
`derive_superseded_branches`, etc. all live here too). This is consistent
with the file's existing organization -- no new architectural pattern is
introduced -- but it's worth flagging that this file continues to grow as a
single monolith rather than being split by command family. Out of scope for
this issue (a file-organization refactor was never part of this design or
plan), and not something this fix should have taken on unilaterally.

## Verdict

`blocking_count: 0`, `advisory_count: 1` (pre-existing file-size/organization
concern, not introduced or worsened meaningfully by this change, out of
scope for this issue).
