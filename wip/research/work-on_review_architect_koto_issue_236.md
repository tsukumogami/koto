# Architect review: koto#236 / #200 (state-log header guard)

Diff reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'` (commits fddc4aa, d52dd0f, 1b6dcd1).
Verified: `cargo check --all-targets` clean; `cargo test --test claim_sidecar` (21 pass);
`cargo test --lib request_store` (76 pass).

Blocking: 0. Advisory: 8.

## Verdict

The change fits koto's structure. The invariant ("an append never creates a log and never
extends a headerless one") lives in `engine::persistence`, the one funnel every writer goes
through. That includes the writers that bypass `SessionBackend`: `cli/session.rs:494`
(`session update`), `engine/claim.rs:626,677`, `engine/respawn.rs:548,569`, `engine/wake.rs:336`,
and the request store. Putting the guard in the backend would have missed all of them. The CLI
adds two guards above it, and each has its own job. The dispatch-level `backend.exists` check
(`src/cli/mod.rs:1426`, `:1486`) picks the exit code (2, the same pattern as `handle_rewind` at
`src/cli/mod.rs:2152` and `status`). The handler-level `backend.read_header` (`src/cli/context.rs:26`,
`:149`) orders the header check before the store mutation, so a refusal leaves `ctx/` untouched.
The dependency direction doesn't change: persistence uses only engine types (`EngineError`,
`LogHeader`), and `request_store` is engine code picking an engine generic. No
engine-to-session or engine-to-cli edge was added.

The generic `append_event_in::<H: LogHeader>` / `append_event_idempotent_in::<H>` is the right
shape. The old `read_last_seq` skipped line 1 without looking at it, so the request store could
share the code without saying which header family it writes. Checking the header forces that
choice, and `src/engine/request_store/mod.rs:1256` now states it (`RequestHeader`). The
non-generic `append_event` / `append_event_idempotent` default to `StateFileHeader`, which keeps
every session-log caller source-compatible.

Nothing below violates layering or leaves a contract inconsistent in a way that compounds.
The advisories are stale doc comments on the stability-tracked trait, one unchecked
read path inside the idempotent append, a dead fallback in claim.rs, and notes for the
follow-up lock design.

## Advisory findings

### A1. The trait-level contract for `append_event` wasn't updated (docs)

- `src/session/mod.rs:249-256`: `SessionBackend::append_event` still says only "Append an event
  to the state file." The refuse-unless-header contract is documented only at
  `src/engine/persistence.rs:134-146`. The trait is the stability-tracked surface bunki imports
  ("additive-only"). Going from "creates if missing" to "errors if missing or headerless" is a
  behavior change for any external implementor or caller, and the trait doc should state the
  precondition ("the session's log must already exist with a header; only `init_state_file` /
  `append_header` create one").
- `src/session/local.rs:282`: "Match append_header/append_event which create state files with
  mode 0600" is now wrong about `append_event`. It no longer creates anything, and the `mode(0o600)`
  was removed from both append paths. Only `append_header` creates now.
- `src/engine/persistence.rs:275-276`: "`None`: identical to [`append_event`]. The event is appended
  unconditionally" reads as if nothing can refuse it. "Identical to `append_event`, including its
  header check" would be accurate.
- `src/session/cloud.rs:729-737`: `CloudBackend::append_event` appends to the local copy without
  pulling first. It therefore has an implicit precondition, "the local copy exists", that isn't
  written down anywhere. See A5.

### A2. The idempotent hash scan still skips line 1 unseen, before the header check

`src/engine/persistence.rs:336-372`: in `append_event_idempotent_in`, the hash-hit scan runs
`content.lines().skip(1)` (line 340) and can return `Idempotent` or `ConcurrentSubmissionConflict`
before `next_seq_after_header::<H>` (line 376) ever looks at the header. Nothing is written on
those returns, so this can't corrupt a log. But it's the same unchecked skip the fix removed from
`read_last_seq`, so one append path now answers a headerless log with a normal result while the
other refuses it. `if path.exists()` at line 336 is also dead now:
`acquire_state_flock` (line 406) opens without `create`, so a missing file has already failed.
Suggestion: read the file once under the flock, check the header first, then do the hash scan and
the seq derivation from that one read. That's one read instead of two, and one site for the
follow-up lock to cover.

### A3. Error-type split: defensible, but the checks and wording are duplicated

Missing file gives a plain `anyhow` error. Empty file, empty first line, and a first line that
isn't a header give `EngineError::StateFileCorrupted`, which maps to exit 3
(`src/engine/errors.rs:184`). That matches `read_header_only` (`persistence.rs:582-600`): the
missing file is a lifecycle/caller condition and the CLI turns it into exit 2 before it reaches
the engine, while the rest is data corruption. Two small points:

- `next_seq_after_header` (`persistence.rs:525-567`) re-implements the empty and empty-first-line
  checks from `read_header_only` (`:590-599`) with different message text ("..., so nothing was
  appended to <path>"). A shared `parse_first_line::<H>(content: &str)` helper would keep the two
  readers from drifting.
- `header_parse_failure` (`persistence.rs:627-641`) runs for every `H`, so a request log whose
  first line is an event gets "The session was never initialized, or its log was recreated after
  the session was removed". That wording is for session logs. The request store's own
  `NotFound`/symlink checks make this very unlikely to show up, so it's cosmetic.

### A4. Where the guard lives: correct, with one note on `ContextStore::add`

There are three levels of guard: CLI dispatch (exit-code choice), CLI handler (store ordering),
and engine persistence (the invariant). They aren't redundant. Without the handler check, the
store would already hold the content by the time the engine refuses. Without the engine check,
the other writers listed in the verdict go unprotected. The session-layer `ContextStore::add`
(`src/session/local.rs:497`, `create_dir_all`) still accepts any session name. Its other
production caller, `workflows_surface::discover::publish_location` (`src/workflows_surface/discover.rs:37-43`),
never appends an event, so it can't produce the #236 shape. Leaving the store unguarded is
consistent with the approved scope. Recorded here so the follow-up doesn't assume the store
checks anything.

### A5. Cloud: strictly better, with a precondition that isn't written down

`CloudBackend::exists` returns true for an S3-only session (`src/session/cloud.rs:686-692`).
For the context verbs, `backend.read_header` pulls first (`cloud.rs:751-754`), so the local log
exists by the time the append runs. Other cloud appenders don't pull. For example,
`append_child_completed_to_parent` (`src/cli/mod.rs:2556-2585`) checks `exists` and then calls
`backend.append_event`. Before this change, that path created a headerless local file and
`sync_push_state` then pushed it over the good S3 object. Now the append refuses and the caller
takes its `AppendFailed` branch. That's an improvement, and it's a nice side effect of putting the
guard in persistence. But `CloudBackend::append_event` doesn't document that it needs a local
copy (A1). Recommend the follow-up design, which already owns the cloud pull, decide whether
`append_event` should pull on a local miss.

### A6. Seam for the follow-up write lock: workable, but the check still runs twice

What the follow-up has to work with:

- The header check (`next_seq_after_header`) and the append (`OpenOptions::append`) are two
  separate opens inside `append_event_in` (`persistence.rs:151-185`). The context verb adds a third
  read: `read_header` (`cli/context.rs:26`), then `store.add`, then `append_event`, which reads the
  header again. The follow-up note already says the lock has to span the check, the store write
  and the append, and that it has to be passed down rather than taken again inside (flock handles
  conflict within one process).
- Nothing in the current signatures (`append_event(path, payload, ts)` and
  `SessionBackend::append_event(id, payload, ts)`) can carry a held lock. That's fine for this PR.
  The `_in::<H>` split shows where a locked variant goes: a sibling
  `append_event_locked_in::<H>(&StateWriteGuard, path, ...)` with the current functions as thin
  wrappers. The design should also decide what happens to `acquire_state_flock`
  (`persistence.rs:406`, flock on the log inode). It overlaps with a sidecar lock, is orphaned by
  rename-replace, and the request store already takes `request.lock` on top of it
  (`request_store/mod.rs:1223`), so request appends lock twice today. That's pre-existing.
- Once the verb holds the lock, the handler's `read_header` pre-check could be folded into the
  locked append ("check header, mutate store, append" under one guard). The follow-up design
  should say explicitly whether the pre-check stays.

Worth adding to `wip/followup_state-log-write-lock.md` before it's copied somewhere durable: A2
(single read under the lock) and the double-lock note above.

### A7. The claim.rs `.audit.jsonl` fallback is now dead code

`src/engine/claim.rs:592-610`: `coord_state_file_for` falls back to
`<session_dir>/<coord_id>.audit.jsonl`, a file that never gets a header, "so tests can still assert
the call" (line 604). Under the new contract `append_redelegated_audit` (`:612-630`) always fails
on that path, so the comment is now false. `recover_orphaned_sidecar` has no production caller
(only `tests/claim_sidecar.rs`, 21 tests pass, so none of them reach the fallback), and the
follow-up note lists the misnamed-path bug. Not blocking. Recommend deleting the fallback, or
leaving a TODO pointing at the follow-up issue, so nobody wires recovery up and finds the audit
append failing.

### A8. The exit-code path for context verbs is consistent, with one edge case

After the dispatch check passes, any handler error maps to `EXIT_INFRASTRUCTURE` (3)
(`src/cli/mod.rs:1435-1444`, `:1495-1504`). So a session cleaned up between the dispatch
`exists` check and the handler's `read_header` exits 3 ("failed to open state file"), not 2. That
window is the TOCTOU the follow-up note already covers. The docs
(`docs/reference/error-codes.md` new "context add and context remove" section,
`command-reference.md:782`) say 2 for no log and 3 for a corrupt one, which matches the code
outside that race. No action needed in this PR.

## Things checked and found fine

- Every production caller of `persistence::append_event`, `append_event_idempotent` and
  `SessionBackend::append_event` (grep over `src/`) appends to a log that `init`,
  `init_state_file` or `append_header` created, or already treats an append failure as
  non-fatal (`append_child_completed_to_parent` returns `AppendFailed`). No caller depended on
  create-on-append, apart from the dead claim fallback (A7).
- `read_last_seq` is fully gone; there are no remaining unchecked skips of line 1 outside the
  hash scan (A2).
- Request store: `append_under_lock` checks `path.exists()` (`request_store/mod.rs:1222`)
  before the flock, so `acquire_state_flock` dropping `create` changes nothing there.
- `SessionBackend::exists` is documented as "state file present, not just directory"
  (`src/session/mod.rs:206`), so the dispatch guard catches a directory holding only `ctx/`, which
  is exactly the state the error-codes remedy leaves behind.
