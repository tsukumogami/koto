# Handover: koto#609, declared branch of held_elsewhere

Branch `feature/609-declared-lookup`, cut from main at 76bc56e. No code
changed yet. The work-on session `issue_609` is at `implementation` (review
level `full`, unit kind `change`); its `baseline.md`, `introspection.md` and
`plan.md` are in koto context (`koto context get issue_609 plan.md`).

## Where the code is

- `src/session/cloud.rs`: `check_located` (~798), `located_probe` (~856),
  `held_elsewhere` (~907, the placeholder `let _undeclared = (declaration,
  memo);` at ~938 is where the decision goes), `start_lookup` (~966),
  `sweep` (~987), `write_entry` and its 403 handling (~1542). Helpers near
  ~6759-7011: `is_answered_failure`, `is_transient`, `list_retrying`,
  `asked_retrying`, `list_capped`, `LookupStart`, `IndexRead`, `read_index`,
  `top_level_prefixes`, `any_holds`, `joined`. `self.probes.window` holds
  `check_window_secs` (set ~419).
- `src/session/name_index.rs`: `DECLARATION_KEY`, `MAX_ENTRIES_PER_LOOKUP`,
  `MAX_DECLARATION_BYTES`, `prefixes_from`, `parse_declaration`,
  `DeclarationRead`, `read_declaration` (lists the key, then a capped GET),
  `Memo` (`load`, `save`, `record`; `save` skips a missing directory).
- `tests/cloud_bound_dir_test.rs`: `Host` helper (window 0 on every command;
  set the window per command, don't change the helper), `assert_unlocated`,
  `asked_under`, `prefix_of`. `tests/cloud_name_index_test.rs` has a private
  `declaration()` body and `declaration_reads()`; don't edit them.
- `tests/support/fake_s3.rs`: `put`, `remove`, `requests` (`method`, `key`,
  `list_prefix`, `delimiter`, `range`), `clear_requests`, `fail`,
  `fail_times`, `clear_faults`, `set_latency`; pages at 1000 keys.

## Decided approach

1. Decision 5 in `held_elsewhere`, from the memo loaded before step 1:
   - memo "declared" (any age): don't start the top-level listing;
   - memo "declared", window > 0 and younger than the window: no
     declaration read at all;
   - otherwise read the declaration. With a memo saying "declared", GET it
     directly (capped range, retried through `asked_retrying` on
     `is_transient`), since the object is expected; this is what makes the
     declared lookup exactly 3 requests (own-prefix LIST, index LIST, GET) in
     2 round trips. With no memo or "not declared", keep the list-first
     `read_declaration`, so the undeclared path sends what main sends.
   - Present: declared, record the memo. Absent or invalid: undeclared,
     memo cleared (`Memo::record`). Unreadable: declared only when the memo
     says "declared", the window is above 0 and its `read_at` is under 10
     minutes old; leave the memo as it was (failed reads never move the
     limit). Add a named 10-minute constant.
2. Declared branch: index `Err` -> `Err` (can't tell; `read_index`'s message
   already names `koto-names/<name>/` and carries the 503). Capped -> start
   the top-level listing if step 1 didn't, then today's sweep with nothing
   confirmed absent. Otherwise `any_holds(prefixes)` decides: `Ok(true)`,
   `Ok(false)`, or `Err`. Ignore the top-level result and its failure.
3. Undeclared branch: today's code; start the top-level listing now when
   step 1 skipped it (memo said "declared" but the read cleared it, or the
   carry expired).
4. `write_entry` stays as it is (it reads the bucket, never the memo). Its
   absent-declaration case lists the key and makes no GET, so the
   "removed declaration, memo still declared" test should assert a request
   for `koto-names/_complete` after the 403 rather than a GET; record that
   as a decision.
5. Add a reusable helper that plants a valid declaration body in the fake
   (for #610), e.g. in `tests/support/`.

## Tests to add (names fixed by the issue)

`a_missing_name_on_a_declared_bucket_costs_three_requests_at_any_prefix_count`,
`a_missing_name_on_a_degraded_declared_bucket_asks_a_bounded_number_of_listings`,
`a_declared_bucket_still_refuses_an_indexed_session_from_another_prefix`,
`a_failed_index_listing_on_a_declared_bucket_is_cant_tell`,
`an_unreachable_declared_bucket_still_reads_a_never_used_name_as_absent`,
`an_oversized_or_malformed_declaration_reads_as_undeclared`,
`a_refused_entry_put_blocks_the_push_on_a_declared_bucket`,
`a_name_with_more_entries_than_the_cap_falls_back_to_the_sweep`,
`a_carried_declaration_expires_while_reads_keep_failing`.
Age the memo by rewriting `read_at` in `cloud-index.json` (beside the local
session store), never by sleeping. No existing test function may change; the
issue's Validation script checks that.

## Next steps

Implement, run the named tests and `cargo test --test cloud_bound_dir_test
--test cloud_name_index_test` with output to a file, commit, then submit
`implementation_status: complete` on `issue_609` and continue the work-on
loop. The full local suite runs only in the slot the run's coordinator names.

Delete this file before the pull request merges.
