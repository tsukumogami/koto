# Scrutiny: Completeness

Issue 4 -- feat(cli): surface stale template_source_dir on koto status and koto session list.
Commit reviewed: b53d61a.

## Acceptance criteria vs. implementation

1. Both `handle_status` and `handle_list` call `format_stale_template_source_note(backend.is_cloud())`,
   no new formatting helper defined -- **met**. `derive_stale_template_source_dir` (new, `src/cli/mod.rs`)
   calls `format_stale_template_source_note(is_cloud)` where `is_cloud` is passed in from
   `backend.is_cloud()` at the `handle_status` call site; `handle_list` (`src/cli/session.rs`) calls
   `crate::engine::template_source_status::format_stale_template_source_note(is_cloud)` directly. Neither
   defines a new wording function -- verified via `git diff`, which shows no new `pub fn format_*` or
   similar in this commit.

2. `handle_status` adds a conditional `stale_template_source_dir` key present only when
   `check_template_source_dir(...)` returns `Some(status)` with `exists == false`, matching the
   `batch`/`superseded_branches` present-only-when-relevant convention -- **met**.
   `derive_stale_template_source_dir` returns `None` unless `check_template_source_dir` returns `Some`
   with `exists: false`; `handle_status` only assigns `response["stale_template_source_dir"]` inside an
   `if let Some(stale) = ...` guard, directly adjacent to and following the same pattern as the existing
   `if let Some(batch_view) = ...` / `if !superseded.is_empty()` guards a few lines above.

3. When the check returns `None` or `exists == true`, the response contains no
   `stale_template_source_dir` key at all (not `null`) -- **met**. The helper returns `Option<Value>`,
   and the only assignment site is inside the `if let Some(...)` guard -- there is no code path that
   assigns `serde_json::Value::Null` to the key. Verified directly by
   `derive_stale_template_source_dir_omitted_when_absent` and
   `derive_stale_template_source_dir_omitted_when_existing` (unit tests, `src/cli/mod.rs`) and by
   `status_omits_stale_key_when_template_source_dir_still_exists` (integration test asserting
   `json.get("stale_template_source_dir").is_none()`, i.e. absent rather than `Value::Null`).

4. `handle_list` surfaces `template_source_status` per row (already on `SessionInfo` since Issue 3,
   already `Serialize` since Issue 3) with the wording note attached for rows where `exists == false`,
   gated on the list's own backend via `backend.is_cloud()` -- **met**. `handle_list` converts
   `Vec<SessionInfo>` to `serde_json::Value` (the existing per-row shape is unchanged -- `SessionInfo`'s
   derive already serialized `template_source_status`, so this conversion is not new I/O or a new field,
   just a vehicle for the additive `note` mutation), then for each row where
   `template_source_status.exists == false` injects `status["note"] = format_stale_template_source_note(is_cloud)`
   where `is_cloud = backend.is_cloud()` computed once from the list's own backend.

5. `docs/guides/cli-usage.md` updated: `#### session list` section notes the new field;
   `koto status <parent>` bullet under "Batch surface on existing commands" extended to describe the new
   conditional key -- **met**. Both edits present in the diff. See Justification review for a note on
   why the `#### session list` JSON example block itself was not extended (advisory, not a completeness
   gap -- the existing example already omits other real fields like `parent_workflow`).

6. Tests confirm: key omitted for no-`template_source_dir`/existing-directory cases (unit:
   `derive_stale_template_source_dir_omitted_when_absent` for absent; integration:
   `status_omits_stale_key_when_template_source_dir_still_exists` for existing); present with direct
   wording for stale `LocalBackend` (`status_surfaces_stale_key_with_direct_wording_for_local_backend`,
   `list_surfaces_direct_wording_for_local_backend`); present with softened wording for stale
   `CloudBackend` (`status_surfaces_stale_key_with_softened_wording_for_cloud_backend`,
   `list_surfaces_softened_wording_for_cloud_backend`); same distinction in `handle_list` (the four
   `list_*` tests above) -- **met**, all six new integration tests plus four new unit tests pass.

7. `cargo build`/`cargo test` pass; `cargo fmt --check` and `cargo clippy --lib -- -D warnings` clean
   (ignoring the pre-existing `--all-targets` clippy errors) -- **met**, ran all four explicitly; full
   `cargo test` (1289+ lib tests plus every integration binary) passes with zero failures; `cargo fmt
   --check` and `cargo clippy --lib -- -D warnings` both produce no output.

## Evidence verifiability

Every claim above is directly checkable from `git show b53d61a` and the command output captured in this
session (`cargo build`, `cargo test`, `cargo fmt --check`, `cargo clippy --lib -- -D warnings`, plus the
targeted `cargo test --test stale_template_source_dir_cli_test` and `cargo test --lib
derive_stale_template_source_dir` runs).

## Verdict

blocking_count: 0
advisory_count: 1 (see justification review: the `#### session list` JSON example block was not
literally extended with the new field, only described in prose -- consistent with existing doc style
but worth a maintainer's eye)

Every acceptance criterion has a corresponding, verifiable implementation change or verification step.
Nothing is missing.
