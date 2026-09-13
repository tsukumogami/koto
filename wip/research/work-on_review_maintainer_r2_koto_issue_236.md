# Maintainer review, round 2 — koto#236 state-log integrity

Scope: round-2 delta `9f08117` on top of the round-1 work, checked against the
code it describes. Range reviewed: `git diff 5b8cfbf..HEAD -- . ':!wip'`.

Verdict: **0 blocking, 3 advisory.** The round-1 blocking finding is fixed, and
every new comment and doc claim I checked holds against the code.

`cargo test --test doc_names` — 24 passed, 0 failed.

## Round-1 blocking finding: resolved

`src/session/local.rs:281-284` now reads:

```
// Creating the state file is this function's job: appends
// refuse a log that isn't already there. Mode 0600 matches
// what `append_header` sets on a file it creates.
```

Both halves check out. `append_event` / `append_event_in`
(`src/engine/persistence.rs:145-176`) open with `.append(true)` and no
`create`, and go through `next_seq_after_header` first
(`src/engine/persistence.rs:528-568`), so an append cannot bring a log into
being. `append_header_line` (`src/engine/persistence.rs:87-98`) still opens
with `create(true)` and `opts.mode(0o600)`, so "mode 0600 matches what
`append_header` sets on a file it creates" is literally true. The false claim
("append_header/append_event create state files with mode 0600") is gone.

## Claim-by-claim verification

**Trait doc, `src/session/mod.rs:255-259`** — "Fails, writing nothing, unless
the log already exists and its first line is a header." Correct for both
implementations: `LocalBackend::append_event`
(`src/session/local.rs:189-203`) delegates to `persistence::append_event`, and
`CloudBackend::append_event` (`src/session/cloud.rs:729-737`) delegates to the
local backend and only then pushes. Note the cloud path does not pull first,
so "already exists" means the local copy — the doc's wording does not overclaim.

**Idempotent-append doc, `src/engine/persistence.rs:275-276`** — "`None`:
identical to `append_event`. The event is appended with no hash field, and the
same header check applies." Correct: the `None` arm at
`src/engine/persistence.rs:321-324` calls `append_event_in::<H>`, which runs
`next_seq_after_header`.

**Header-first comment, `src/engine/persistence.rs:333-337`** — the guard
really is before the scan now (`let next_seq = next_seq_after_header::<H>(path)?;`
at line 338, scan at 344-375), and the dead `if path.exists()` wrapper is gone.
"Take the seq from the same read" is accurate as written (header check and seq
come from one call), though the scan still does its own `read_to_string` — the
same two reads as before this change, so no regression.

**Missing-log error, `src/engine/persistence.rs:531-536`** — no longer credits
init alone: "a session's log is written when the session is created, never by an
append."

**Request-store comment, `src/engine/request_store/mod.rs:1256-1257`** — "A
request log's first line is a `RequestHeader`, not a session header, so the
append checks it against that type." Correct: `append_event_idempotent_in::<RequestHeader>`
parses line 1 as `RequestHeader` via `parse_header`, and `append_under_lock`
already guarantees the path exists (`src/engine/request_store/mod.rs:1223-1227`).

**CLI refusal comment, `src/cli/mod.rs:1422-1430`** — every clause verified:
- "created (init, session start, a batch spawn)" — `init_state_file` callers are
  `src/cli/init_child.rs:628/786/998`, `src/cli/session.rs:374` (`koto session
  start`), and the workflows-surface paths; none of them is an append.
- "Exit 2, as `status` does" — `handle_status` (`src/cli/mod.rs:5835-5844`)
  exits 2 with the identical `workflow '<name>' not found` payload on
  `!backend.exists`.
- "A log that exists but can't be read is exit 3, reported by the handler" —
  `handle_add`'s leading `backend.read_header(session)?`
  (`src/cli/context.rs:23-27`) surfaces `EngineError::StateFileCorrupted`, and
  the call site maps any handler error to `EXIT_INFRASTRUCTURE = 3`
  (`src/cli/mod.rs:1440-1449`, constant at `src/cli/mod.rs:75`).

**`docs/reference/error-codes.md:186`** — the quoted JSON matches the code
exactly: `header_parse_failure` (`src/engine/persistence.rs:630-644`) produces
the "state log has no header: its first line is a `<type>` event (seq N)…"
string, and `EngineError::StateFileCorrupted` prefixes "state file corrupted: "
(`src/engine/errors.rs:19`) and maps to exit 3 (`src/engine/errors.rs:184`).

**`docs/reference/error-codes.md:186` remedy** — the two new clauses are right:
- "Leave the `ctx/` directory in place, so the context already stored there
  survives." Context retrieval reads the on-disk store (`ctx/` plus
  `ctx/manifest.json`, `src/session/local.rs:397-399`,
  `src/session/context.rs:13`), not the event log; `ContextAdded` events are
  consumed only by the dashboard activity view
  (`src/cli/dashboard_data.rs:852`). And a respawn is not blocked by a
  surviving `ctx/`: `init_child.rs` creates the session directory first and the
  atomic rename is fail-if-exists on the state file alone
  (`src/session/local.rs:294-303`).
- "Under the cloud backend, remove the headerless copy from the remote store
  too, or the next read pulls it back down." Correct and worth saying:
  `sync_pull_state` (`src/session/cloud.rs:150-164`) unconditionally overwrites
  the local state file on every `read_events` / `read_header`, and
  `CloudBackend::exists` (`src/session/cloud.rs:686-692`) falls back to S3, so
  deleting only the local file does not clear the condition.

**`docs/reference/error-codes.md:230`** — "pass the input through the task
entry's `vars`" is a real field (`src/engine/batch_validation.rs:100`).

**`docs/guides/cli-usage.md:459` and `:527-529`** — exit 2 with nothing stored
is what the code does (the `backend.exists` gate runs before `handle_add` /
`handle_remove` touch the store, `src/cli/mod.rs:1431-1438` and `1487-1496`;
`LocalBackend::exists` keys on the state file, not the directory,
`src/session/local.rs:81-83`, so a directory holding only `ctx/` still refuses).

**`plugins/koto-skills/skills/koto-user/references/batch-workflows.md:78`** —
sits directly under the `materialized_children[*].outcome` table, where
`pending` and `blocked` do mean "no state file", so the sentence is correctly
scoped.

**CHANGELOG counts** — "Eight integration tests … and five unit tests" matches:
8 `#[test]` in `tests/state_log_integrity_test.rs`, 5 new unit tests in
`src/engine/persistence.rs:1846-1930`.

## Advisory findings

**A1 — `src/engine/persistence.rs:409-411` still says "only init creates a
log".** Round 2 deliberately broadened that phrasing in the user-facing error
(`src/engine/persistence.rs:534`) and in the CLI comment ("init, session start,
a batch spawn", `src/cli/mod.rs:1423-1424`), but the `acquire_state_flock`
comment and the `append_event` doc at `src/engine/persistence.rs:139` still
attribute creation to `init` alone. Reads as shorthand for the `init_state_file`
path rather than as a false statement, so this is consistency, not correctness.
Suggested: "only session creation writes a log".

**A2 — "still `pending` or `blocked`" is surface-dependent and
`docs/guides/cli-usage.md:459` does not name the surface.** In
`materialized_children[*].outcome` the claim is exact. In the
children-complete gate aggregate, `TaskOutcome::Running` is projected to
`pending` (`src/cli/batch.rs:2431-2443`), so a reader watching that surface can
see `pending` for a child that does have a log and would accept `context add`.
Same applies to `docs/reference/error-codes.md:230`'s "wait until its `outcome`
is `running`" — `running` never appears in the gate aggregate. Both are correct
against the surface `cli-usage.md:1039` documents; naming it
("in `koto workflows --children`") would remove the ambiguity.

**A3 — nit, `src/engine/persistence.rs:338-346`.** The idempotent path reads the
file twice (`next_seq_after_header`, then the scan's `read_to_string`). That is
the same read count as before the change, so nothing regressed, but the two
reads are now adjacent and one `read_to_string` could serve both if anyone
touches this again.
