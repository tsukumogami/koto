# Architect review, round 2 — koto#236 state-log integrity

Scope: `git diff 5b8cfbf..HEAD -- . ':!wip'`, with the round-2 delta `9f08117`
read closely. Read-only review.

Verdict: **0 blocking, 6 advisory.** The layer contracts are consistent for
every backend path a production caller reaches today. The one gap that is
genuinely a contract ambiguity (CloudBackend) is fail-closed rather than
corrupting, and it is recorded in the follow-up note, so it does not meet the
compounding bar.

---

## What round 2 fixed, verified

- `src/session/mod.rs:255-259` — the `SessionBackend::append_event` trait doc
  now states the refuse-unless-header contract and points creation at
  `init_state_file`. Correct and matches both impls.
- `src/session/mod.rs:248` — `append_header`'s doc ("Append a header line to a
  new state file") is still create-semantics, and that is still *true*:
  `persistence::append_header_line` at `src/engine/persistence.rs:87-116` opens
  with `create(true)` and fsyncs the parent dir on first write. No stale claim
  here.
- `src/session/local.rs:281-284` — the `init_state_file` mode-0600 comment no
  longer credits `append_event` with creating files.
- `src/engine/persistence.rs:275-276` — the idempotent doc's `None` branch no
  longer says "unconditionally".
- `src/engine/persistence.rs:334` — the header check moved ahead of the hash
  scan. The dead `if path.exists()` wrapper is gone.
- `src/engine/persistence.rs:533` — the missing-log error no longer says "only
  init creates one".

I grepped `src/` and `docs/` for surviving create-if-missing assertions. The
only hit is item 3 below; everything else is either corrected or was never
wrong.

---

## Lock discipline in the restructured idempotent path — sound

`src/engine/persistence.rs:329-397`. The order is now:

1. `acquire_state_flock(path)?` bound to `_guard` (a named binding, not `_`, so
   the `File` lives to end of scope and the advisory `LOCK_EX` is held).
2. `next_seq_after_header::<H>(path)?` — read #1, header validated, seq taken.
3. `read_to_string` — read #2, the hash scan.
4. `OpenOptions::append().open(path)` + `writeln!` + `sync_data`.

Every early return (`AppendOutcome::Idempotent`, `ConcurrentSubmissionConflict`,
each `?`) drops the guard on the way out, and no path writes after the drop.
The whole read-then-write window is inside the lock, so the seq taken at step 2
cannot go stale before step 4 relative to another idempotent writer. Moving the
header check *inside* the lock (it was already inside, just after the scan) is
strictly safer than the round-1 shape, because the seq and the header now come
from the same `read_to_string` rather than from two reads straddling the scan.

`acquire_state_flock` opening without `create` (`src/engine/persistence.rs:411-417`)
is the right call and is what keeps the lock acquisition itself from
manufacturing the empty log the header check exists to refuse.

The known hole — `append_event_in` (`src/engine/persistence.rs:151-187`) takes
no lock at all, so its header-check-then-append is a TOCTOU against a
truncating writer — is unchanged by this diff and is the first item in
`wip/followup_state-log-write-lock.md`.

## The follow-up seam — right shape

`wip/followup_state-log-write-lock.md` is the right artifact and names the right
things: the check-and-append must be spanned by one lock; the lock belongs at
the context-verb level so it also covers `store.add`; a flock taken twice on two
handles in one process self-conflicts, so the verb's lock has to be *passed
down* rather than re-taken inside the backend; and the existing
`acquire_state_flock` overlaps whatever sidecar lock the design lands on. It
also correctly records that the context verbs currently check the header twice
and that one check suffices under a spanning lock. Nothing in the seam looks
mis-scoped. One caveat, item 6 below: the note says "copy it somewhere durable
before the branch merges", and under the wip-hygiene rule it will be deleted, so
that copy has to actually happen.

---

## Advisories

### A1. `CloudBackend::append_event` now requires a local copy; nothing says so, and `exists` disagrees

`src/session/cloud.rs:729-738` delegates straight to `self.local.append_event`
and then pushes. With the new guard, that delegation fails unless the state log
is present **locally** with a valid header. But `CloudBackend::exists`
(`src/session/cloud.rs:686-692`) returns `true` for a session that exists only in
S3, and `read_events`/`read_header` (`src/session/cloud.rs:740-754`) pull first
while `append_event` does not.

So `SessionBackend::exists` and `SessionBackend::append_event` now use two
different notions of "the log exists" on the cloud backend, and the trait doc's
"unless the log already exists" (`src/session/mod.rs:256`) does not say which one
it means. A future backend implementer reading that sentence could reasonably
pull-then-append, or check remote existence, and be conformant either way.

Reachability today is limited, and the reason is incidental rather than
designed: `context::handle_add` and `handle_remove` now call
`backend.read_header(session)` first (`src/cli/context.rs:24`,
`src/cli/context.rs:150`), and on `CloudBackend` that runs `sync_pull_state`. So
the context verbs pull the remote log down before the append reaches the guard.
That is load-bearing behaviour resting on a side effect of a read, with no
comment saying so at either end.

Where it still bites: a cold cloud machine whose local session directory does
not exist. `sync_pull_state` (`src/session/cloud.rs:150-165`) writes with
`std::fs::write` and no `create_dir_all` (unlike `write_local_state_bytes` at
`src/session/cloud.rs:542-552`), so the pull warns and drops, `read_header`
fails on the missing local file, and `context add` exits 3 "corrupt" for a log
that is intact in S3.

Not blocking, and not a regression: on `main` that same path created a
headerless local log and `sync_push_state` clobbered the good remote log with
it — koto#236 at its worst. The change converts silent remote data loss into a
loud, if misleading, failure. Fail-closed ambiguity does not compound.

Fix: two sentences on `CloudBackend::append_event` saying it appends to the
local copy and does not pull, and one on the trait doc saying "exists" means
whatever storage that backend's `read_header` reads. Optionally add
`create_dir_all` to `sync_pull_state`.

### A2. Round 2's improved missing-log message is unreachable on the idempotent path

`src/engine/persistence.rs:533` was rewritten to explain that a log is written
when the session is created, never by an append. On `append_event_idempotent_in`
with `hash = Some(_)`, the flock is now taken first
(`src/engine/persistence.rs:330`), and `acquire_state_flock` opens without
`create`, so a missing file fails there with the bare
`"failed to open state file for lock {}: ..."` (`src/engine/persistence.rs:418-424`)
and never reaches `next_seq_after_header`.

Net: the same function returns two different missing-file errors depending on
whether a hash was supplied — the good guidance when `hash` is `None` (it routes
through `append_event_in`), the bare lock-open error when it is `Some`. The
request store is the only caller (`src/engine/request_store/mod.rs:1258`) and it
wraps the text into `RequestStoreError::Other("append failed: {e}")`, so this is
operator-facing text quality, not behaviour. Worth folding the same sentence
into the flock-open error.

### A3. `acquire_state_flock`'s own comment still credits init alone

`src/engine/persistence.rs:411` — "only init creates a log". Round 2
deliberately broadened that phrasing everywhere else (the CLI comment at
`src/cli/mod.rs:1422-1424` now says "init, session start, a batch spawn", and so
does the error at `src/engine/persistence.rs:533`). This one comment was missed
while its two siblings were fixed, which is the kind of drift that makes the
next reader unsure which statement is authoritative. One-line fix.

### A4. `src/engine/claim.rs` still carries create-if-missing residue

Two call sites prepare a directory for a file the append will no longer create:

- `src/engine/claim.rs:622-626` — `fs::create_dir_all(parent)` then
  `append_event(coord_log, ...)`.
- `src/engine/claim.rs:670-677` — the same pattern in `claim_and_dispatch`.

Combined with `coord_state_file_for` (`src/engine/claim.rs:592-609`) computing
`<coord_id>.state.jsonl` rather than the real `koto-<id>.state.jsonl`
convention, the primary branch never matches and the `.audit.jsonl` fallback
file never exists, so these appends are now unconditionally refused. Confirmed
no production caller: `claim_and_dispatch` and `append_redelegated_audit` have
no references outside `src/engine/claim.rs`. The follow-up note records the
filename bug and the refusal; it does not name the now-pointless `create_dir_all`
pair, which is the visible tell that the file still assumes appends create.
Leaving it is defensible for this PR, but the residue should go in with the
filename fix before the recovery path is wired up.

### A5. The idempotent path reads the whole log twice under the lock

`src/engine/persistence.rs:334` (`next_seq_after_header`, which does its own
`read_to_string` at `src/engine/persistence.rs:529`) and
`src/engine/persistence.rs:344` (the scan's `read_to_string`). Both are inside
the flock so there is no consistency problem — it is a doubled read of the whole
file per append. Request logs are small and this is not a correctness issue, but
the restructure had the content in hand and could have threaded it through
rather than re-reading. Cheap to fix if the lock design touches this function
anyway.

### A6. Adjacent, pre-existing: the non-unix flock stub's comment contradicts its body

`src/engine/persistence.rs:437-444` — the comment says "falling through produces
correct semantics in the no-contention case", but the body returns `Err`
unconditionally and the caller propagates with `?`, so
`append_event_idempotent_in` with a hash cannot succeed at all on non-unix. Not
introduced here and koto is effectively unix-only, but it sits three lines from
code this PR restructured and is the same class of stale-comment problem round 2
set out to clear.

---

## Acknowledged, no action needed in this PR

The context verbs now check the header at three layers: `backend.exists` at
`src/cli/mod.rs:1431` and `src/cli/mod.rs:1490`, `backend.read_header` at
`src/cli/context.rs:24` and `src/cli/context.rs:150`, and
`next_seq_after_header` inside the append. That is redundant but each check
serves a different exit code (2 for "no session", 3 for "unreadable log") and
the redundancy is explicitly on the follow-up note's list for the lock design to
collapse. Exit-code mapping checked and consistent: `EXIT_CALLER_ERROR = 2`,
`EXIT_INFRASTRUCTURE = 3` (`src/cli/mod.rs:75`, `src/cli/mod.rs:80`), and a
headerless-but-present log correctly falls through `exists` to the handler and
exits 3, matching the new text in `docs/reference/error-codes.md`.

`tests/state_log_integrity_test.rs` covers the local backend only — no cloud
path test. Consistent with A1; the cloud behaviour is untested as well as
undocumented, which is worth a line in the PR description even if no test is
added.
