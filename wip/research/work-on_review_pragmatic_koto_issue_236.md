# Pragmatic review: koto #236 / #200 (state log integrity)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'`
Verified: `cargo test --lib` (1572 pass), `cargo test --test state_log_integrity_test` (8 pass),
`cargo test --test claim_sidecar` (21 pass).

Verdict: no blocking findings. The core change is small and well aimed. The
generic `_in::<H>` variants are justified: the request store appends to a log
whose first line is a `RequestHeader`, so a `StateFileHeader`-only check would
break it (request_store/mod.rs:1256). The `acquire_state_flock` and
`OpenOptions` edits remove code rather than add it. What's left is minor
duplication and a little test redundancy.

## Blocking

None.

## Advisory

1. **The "session must have a readable log" precondition is split across two files and three reads.**
   src/cli/mod.rs:1426-1434 and 1484-1494 run the same inline `exists` check, then
   src/cli/context.rs:26 and 149 each call `read_header`, and then the append re-reads
   and re-parses the header (persistence.rs:525). On the cloud backend each
   `read_header` also does a `sync_pull_state` (cloud.rs:751), and `exists` can hit S3.
   Fix: add one `fn require_session_log(backend, session) -> Result<(), NotFound|Corrupt>`
   in context.rs that does both the exists check and the header check. Call it at the top
   of both handlers and let mod.rs map `NotFound` to exit 2, which replaces the two
   11-line blocks. (Inline not-found blocks are already the convention in mod.rs, e.g.
   2150, 5559, 5835, so leaving them is defensible.)

2. **`append_event_in` doesn't need to be `pub`.** persistence.rs:151. Its only callers
   are `append_event` and `append_event_idempotent_in`, and the request store uses the
   idempotent variant. Fix: make it private, or `pub(crate)` at most.

3. **`next_seq_after_header` repeats `read_header_only`'s first-line checks.**
   persistence.rs:538-553 re-implements the empty and empty-first-line errors from
   persistence.rs:586-599. It also branches to produce two messages where one would do.
   Fix: pull out a shared `header_from_first_line::<H>(content: &str)` for both to use,
   or collapse to a single "state file has no header line" message.

4. **Leftover `create_dir_all` and fallback in claim.rs are now dead or always-failing.**
   engine/claim.rs:622-625 and 673-676 create the log's parent directory right before an
   `append_event` that now refuses unless the file already exists. The `.audit.jsonl`
   fallback at claim.rs:601-609 can only fail now, and since `coord_state_file_for` looks
   for `<coord>.state.jsonl` instead of `koto-<coord>.state.jsonl`, it's the path taken
   every time. There's no production caller, and wip/followup_state-log-write-lock.md
   records this. Fix: either drop both `create_dir_all` calls and the fallback now, or
   leave them for the follow-up as planned. Just don't let the follow-up lose track of them.

5. **The existence check runs before key validation, which isn't what the approved scope
   says.** The scope says "before `store.add`, after key validation", but mod.rs:1426
   runs before `handle_add`, and the key is validated inside `store.add`. So
   `context add <missing> ../x` reports not-found rather than an invalid key. That's
   harmless, but it's a quiet deviation. Fix: accept it and say so in the PR body, or
   validate the key first.

6. **Test redundancy: `repeated_context_add_to_a_missing_log_is_refused_each_time`.**
   tests/state_log_integrity_test.rs:156-165. The first refusal leaves nothing behind,
   so the second call takes exactly the same path as
   `context_add_refuses_a_never_initialized_session` (143-151). Fix: delete it.

7. **Test bloat: two near-identical parent templates.**
   tests/state_log_integrity_test.rs:250-327. `REWINDABLE_PARENT_TEMPLATE` is
   `PARENT_TEMPLATE` plus a `gather` state and a different transition, about 40
   duplicated lines. Fix: use the rewindable template in both tests; the blocked-child
   test needs one extra `koto next parent` to get into `plan`.

8. **CHANGELOG entry carries test counts.** CHANGELOG.md, the #236 entry ("Eight
   integration tests ... and five unit tests"). Test counts are noise for release notes.
   Fix: drop that sentence.

## Checked and fine

- `header_parse_failure` (persistence.rs:627) is scope item 3 and stays self-contained.
- The five unit tests (persistence.rs:1834-1930) each pin a separate refusal case: a
  missing file, an empty file, a headerless file, the idempotent path's missing file,
  and the reader's message.
- The blocked-child, terminal-cleanup, and rewind integration tests all hit the same
  mod.rs branch. The scope explicitly required the blocked-child and rewind ones, and
  each one pins a reported user scenario, so they aren't bloat.
- tests/integration_test.rs: swapping `create_session_dir` for `init_context_session`
  is the minimal fix scope item 5 asked for.
