# Handover: koto#608 (index-first lookup on undeclared buckets)

Stopped at the brief's context line (measured 251K > 180K). Pick up from here.

## Where things stand

- Branch `feature/608-name-index-lookup`, based on main `e18c145` (#614).
- koto session `issue_608` (work-on, shirabe 0.43.0), state `implementation`, review level
  `full`, unit kind `change`, dispatcher linked (`coordinate-roadmap-koto-20261108T084214Z`).
  Context keys written: `context.md`, `baseline.md` (from main's hosted CI, no local suite
  run), `introspection.md` (outcome `approach_updated`), `plan.md`. Read them with
  `koto context get issue_608 <key>`.
- The measurement before read for this change is done and posted (target `fleet`, recorded with
  `--repo koto --branch feature/608-name-index-lookup`). Once the PR exists, run the measurement
  skill's link step for it.
- No PR, no tests written yet, nothing run but `cargo build`.

## Code written (builds clean, cargo fmt applied)

`src/session/cloud.rs`:
- `held_elsewhere` rewritten in three steps; `start_lookup` (three scoped threads: `read_index`,
  `name_index::read_declaration`, `top_level_prefixes`); a marked place where the declaration
  read and the memo are held and ignored (for #609); top-level failure returns `Err` before any
  confirming listing; confirmation via `any_holds` unless the index failed or `capped`; `sweep`
  over top-level minus confirmed-absent, printing `SLOW_SWEEP_HINT` on stderr after
  `SLOW_SWEEP_HINT_AFTER` (2 s).
- Free helpers: `asked_retrying` (the retry loop pulled out of `list_retrying_when`),
  `list_capped` (per-page retries, stops at `MAX_ENTRIES_PER_LOOKUP` keys), `LookupStart`,
  `IndexRead`, `read_index`, `top_level_prefixes` (today's error text kept),
  `any_holds` (the old fan-out moved verbatim), `joined`.
- `REQUEST_TIMEOUT` (60 s) set in `create_bucket` with `with_request_timeout`.

`src/session/name_index.rs`: `MEMO_FILE` (`cloud-index.json`), `Memo { endpoint, bucket,
declared, read_at }` with `load`, `save` (temp file + rename, mode 0600) and `record`
(Present -> declared, Absent -> not declared, Unreadable -> unchanged); `memo_endpoint` strips
userinfo with or without a scheme. The memo lives in the local store's journal root.

## Still to do

1. Unit tests: `create_bucket` timeout next to
   `create_bucket_addresses_the_bucket_path_style_when_asked` (with and without path_style);
   memo round trip, garbage file, other bucket, userinfo stripped, mode 0600, Unreadable keeps
   the old value; `prefixes_from` for `foo` over `koto-names/foobar/<p>` returns nothing.
2. New `tests/cloud_index_lookup_test.rs` with every named test in the issue (eight), plus: the
   cap (1,100 planted entries, one index LIST per attempt, reply unchanged), planted valid
   declaration changes nothing and both step-1 requests still sent on a second lookup, garbage
   and foreign memo ignored, request count `3 + d + N` (absent declaration costs one
   `LIST koto-names/_complete`, see introspection), step-1 timing within one latency, the
   slow-sweep hint (fake latency ~700 ms over ~200 prefixes) and no hint on a fast sweep or an
   index hit. Fault tests fail both `LIST koto-names/_complete` and, with a planted declaration,
   `GET koto-names/_complete`. Note `FakeS3::hold` matches object keys only, not listings.
3. `tests/doc_names.allow`: a `promised` record for `session index audit`,
   issue `tsukumogami/koto#610`.
4. Run the cloud test binaries; teach any existing full-request-list test the two new requests.
   `tests/cloud_bound_dir_test.rs` must stay byte-identical.
5. CHANGELOG entry; skills assessment (no CLI surface change expected).
6. Then the workflow: commit, tick `implementation_status: complete`, ask the coordinator for the
   suite slot before verification, panels (full), PR, CI, review round record, READY digest.
