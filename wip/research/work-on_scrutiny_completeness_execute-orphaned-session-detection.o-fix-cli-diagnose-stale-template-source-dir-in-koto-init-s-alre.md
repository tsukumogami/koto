# Scrutiny: Completeness

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Acceptance criteria vs. implementation

1. **Both collision paths open the colliding session's header (new I/O for the
   pre-check) and call `check_template_source_dir`** -- **met**. The new
   `stale_template_source_dir_clause(backend: &Backend, name: &str)`
   (`src/cli/mod.rs`) calls `backend.read_header(name)` then
   `check_template_source_dir(&header)`. It is called from both the
   `backend.exists(name)` pre-check block (`handle_init`, ~line 1721, was a
   pure `exists()` check before this commit) and the
   `SpawnErrorKind::Collision` match arm (~line 1755).

2. **When stale (`exists: false`), both paths append the same staleness clause
   to their existing base message; base messages stay different** -- **met**.
   Both call sites build a `base` string identical to the pre-commit text
   (verified via diff: the pre-check's `base` is character-for-character the
   old format string; the Collision handler's `base` is
   `format!("workflow '{}' already exists", name)`, also unchanged), then do
   `match stale_template_source_dir_clause(...) { Some(clause) =>
   format!("{}{}", base, clause), None => base }`. Since both call the exact
   same function with the same `(backend, name)` shape, the appended clause is
   identical by construction, not by two independently-written format calls
   that could drift. `stale_template_source_dir_clause_identical_across_both_collision_paths`
   (unit test) directly proves determinism (two calls, same result).

3. **Wording calls `format_stale_template_source_note(backend.is_cloud())`,
   no separate accessor/helper defined here** -- **met**. The one call site
   inside `stale_template_source_dir_clause` is
   `format_stale_template_source_note(backend.is_cloud())`. No new formatting
   or `is_cloud`-equivalent function is defined in this commit; `grep -n "fn
   format_\|fn is_cloud" src/cli/mod.rs` after the change shows no new
   matches beyond the existing Issue 1/4 definitions in `session/mod.rs` and
   `engine/template_source_status.rs`.

4. **When not confirmed (`None` or `exists: true`), neither message
   changes** -- **met**. `stale_template_source_dir_clause` returns `None` in
   both cases (no early return before the `status.exists` check other than
   the two `?` on `read_header`/`check_template_source_dir`, and an explicit
   `if status.exists { return None; }`), and both call sites' `match` only
   mutates the string on `Some`. Covered by
   `stale_template_source_dir_clause_none_when_directory_exists` (unit) and
   `init_collision_omits_clause_when_template_source_dir_still_exists`
   (integration, asserts `msg.starts_with(...)` the exact pre-existing prefix
   and `!msg.contains(DIRECT_NOTE)`).

5. **Best-effort header read: a failed read still surfaces the existing
   "already exists" error, not a crash or different error** -- **met**.
   `backend.read_header(name).ok()?` converts any `Err` to early-`None`
   inside the helper; neither call site propagates or unwraps that error, so
   a corrupt state file cannot panic or change the error type -- the
   surrounding `exit_with_error` call with the base "already exists" message
   still fires. Covered directly by
   `stale_template_source_dir_clause_none_when_header_unreadable` (calls the
   helper against a name with no session at all, i.e. `read_header` errors,
   asserts `None`).

6. **Repro test for tsukumogami/koto#189** -- **met**.
   `init_collision_diagnoses_stale_template_source_dir`
   (`tests/stale_template_source_dir_cli_test.rs`) inits a session with
   `--template` pointing at a real directory, deletes that directory, re-runs
   `koto init` with the same name, and asserts the JSON `error` field both
   starts with the unchanged base message and contains
   `"template source directory no longer exists"` plus the canonicalized
   missing path. This is an end-to-end run of the actual `koto` binary
   (`assert_cmd::Command::cargo_bin("koto")`), not a unit-level shortcut, and
   it fails without this commit's changes (the pre-check previously never
   read the header at all).

7. **`cargo build`/`cargo test` pass; existing message-text tests updated
   only where staleness is part of the fixture, otherwise unmodified** --
   **met**. `git diff` shows no modifications to
   `init_duplicate_error_message_is_stable`,
   `init_duplicate_error_mentions_remediation`, or
   `init_child_duplicate_name_rejected` in `tests/integration_test.rs` --
   none of the three create a `template_source_dir` recorded via a real
   `--template` directory that later gets deleted, so staleness never
   applies to them and their fixtures are untouched. Ran full `cargo test`:
   all binaries pass, 0 failures, including the three unmodified tests above
   and the new `stale_template_source_dir_cli_test.rs` (9/9 pass, 3 new).

8. **`cargo fmt --check` and `cargo clippy --lib -- -D warnings` clean** --
   **met**. Both ran with zero output/exit 0. Also spot-checked
   `cargo clippy --all-targets` (not required by the AC, which explicitly
   scopes to `--lib`): 30 pre-existing warnings, all in `src/cli/batch.rs`
   (`useless_vec` lint), none in `src/cli/mod.rs` or the new test file.

## Verdict

`blocking_count: 0`, `advisory_count: 0`. Every AC line item has a
corresponding, verifiable implementation change and a test exercising it
(unit-level for the shared helper's edge cases, integration-level for the
actual bug repro). No evidence claim in the implementation rationale is
unverifiable from the diff.
