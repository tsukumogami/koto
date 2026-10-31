---
schema: design/v1
status: Planned
problem: |
  koto's dashboard reads only this machine's session store and the current
  directory's bucket prefix, so a session another machine wrote is a
  state-less placeholder or isn't listed at all, and nothing records which
  machine produced a session. Reading another machine's state today means
  downloading its whole event log, and the only cross-workspace listing
  keeps nothing between commands.
decision: |
  Record a machine name (KOTO_MACHINE_NAME, else user-level machine.name,
  else the short hostname) on each session's workflow_initialized event, and
  on session_imported for an import. After every push of its own state, the
  writer puts a small summary.json in the session's bucket directory, built
  by the same row derivation the local dashboard uses and carrying the
  machine and the ETag of the state it describes. The reader lists the
  bucket's workspace prefixes, lists each with a /ctx/ delimiter to see every
  session's top-level objects and their ETags, fetches only changed
  summaries, and classifies removed, migrated, legacy and stale sessions.
  The TUI runs this on a background feed every 55 seconds; --once runs it
  once; rows are keyed by session and location; --once appends machine and
  workspace columns.
rationale: |
  Events are append-only, so the machine record survives header rewrites by
  older releases, which a header field wouldn't. The in-directory summary is
  the only placement every release's existing delete already removes, and
  the listing that finds it also returns the state file's ETag and the
  migration marker, so the reader can refuse a stale summary and hide a
  migrated source in a bucket that mixes releases. An index prefix or a
  per-machine index would need new deletes, leave orphans a read-only reader
  can't remove, or need shared writes. A background feed is the only shape
  that keeps bucket requests off the TUI's input thread.
upstream: docs/prds/PRD-cross-machine-dashboard.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: Sessions from every machine in one dashboard

## Status

Planned

## Context and Problem Statement

`koto dashboard` builds its session list in `src/cli/dashboard_data.rs`.
`refresh()` calls `scan_sessions()`, which calls `backend.list()` and reads
each session's state file from a path in this machine's session store
(`backend.session_dir(id)/koto-<id>.state.jsonl`). Under the cloud backend,
`CloudBackend::list()` (`src/session/cloud.rs`) adds the ids it finds in the
bucket under the current directory's workspace prefix that have no local
copy, as `placeholder_session_info(id)` with every field empty. No state is
downloaded for them, so `read_session` finds no local file, marks the row
unreadable with a Unix-epoch mtime, and the row lands in the receded band:
hidden unless the developer passes `--all`. A session under any other
workspace prefix, which is where every other machine's sessions live (the
prefix is the first 16 hex digits of the SHA-256 of the directory koto ran
in), isn't listed at all. And no surface reads a producing machine, because
none is recorded: the header's `origin` (`SessionOrigin` in
`src/engine/types.rs`) holds the anchor directory, the store identity
(kind, base, bucket, prefix) and the anchor epoch, and `machine.id` in the
user configuration is an eight-hex hash of the hostname kept for the
version counter in each session's `version.json`.

The PRD asks for three things this code doesn't do.

First, record a machine name when a session is created (R1 to R5): the
`KOTO_MACHINE_NAME` variable, else `machine.name` in user-level
configuration, else the short hostname, constrained to 1 to 64 characters
of letters, digits, `.`, `-` and `_`. The record has to survive a rebind, a
push and pull, a resolve, and a header rewrite by a release that predates
the feature; an import records the importing machine on its copy and leaves
the source's record alone. A session with no record reads as unknown
(`null` in `--detail`, empty in `--once`, `@unknown` in the TUI), and the
existing `origin` keeps its meaning and shape.

Second, list every other machine's sessions with their current state (R6 to
R13). With the cloud backend selected, the dashboard covers every workspace
prefix in the bucket, follows every page of a listing, filters koto's
internal sessions as it does locally, and shows each remote session's
current state, blocked/failed/terminal flags, intent, template name and
last-change time, so the existing liveness and attention bands apply to it.
Rows are distinct per session copy, not per name; a session with a local
copy appears once (matched by `session_id`); an import's read-only source is
hidden when its copy exists; a session whose state can't be read cheaply is
listed as `unknown` and counted in the unreadable tally; and a session
removed from the bucket drops off.

Third, keep the dashboard cheap, read-only and resilient (R14 to R24).
`--once` appends a `machine` column and a `workspace` column (the 16-hex
prefix) after its eight existing columns; bucket text is sanitized of
control characters in `--once` and the TUI; `--detail` gains a `machine`
field. The remote read uses the credentials koto already resolves, writes
nothing to the bucket or the local store, and works with keys that only
list and get. Every bucket request times out within 10 seconds, failures
are classed (`unreachable`, `denied`, `timeout`, `throttled`) and reported
on one line while local rows stay visible, and the local backend makes no
request at all. The dashboard never downloads a remote session's event log
and reads at most 64 KiB of object data per remote row per refresh, with no
object GET on a refresh where nothing changed. The TUI reflects a remote
change within 60 seconds with up to 200 workspace prefixes, never runs a
bucket request on its input and render thread, and `--once` finishes within
30 seconds for 200 prefixes and 2,000 sessions at 50 ms per request.

Three facts about today's code shape the answer. A full listing of the
bucket costs one listing of the top level plus one per workspace prefix;
the only code that does it today, `held_elsewhere`, fans out 64 at a time
and keeps nothing between commands. The only object that holds a session's
current state is its state file, which is the whole event log: on one
long-lived host the logs run from a few kilobytes to 6.4 MB, 213 MB across
2,322 sessions. And the cloud sync guide currently says an index of
sessions in the bucket "is a storage format change koto doesn't make";
that's a description of today, not a recorded decision, and this design
revisits it.

Session continuity in the bucket (pins, successors, imports, prune, reads at
a session's own copy) is built on as it ships and isn't changed here.
Project-level session-storage settings are out of scope: everything below
reads the bucket configuration koto already resolves at user level.

## Decision Drivers

- **No event log on the read path (R21).** Showing a remote row can't cost
  the size of its history; a refresh where nothing changed reads no object
  data.
- **Freshness and scale (R22, R24).** A change shows within 60 seconds in the
  TUI and on every `--once` run, with 200 prefixes and 2,000 sessions.
- **Read-only reader (R17, R18).** The dashboard writes nothing to the
  bucket or the local store and needs only list and get.
- **The TUI never waits (R23).** Bucket I/O runs off the input and render
  thread; a hanging endpoint costs at most a status line.
- **Survives old writers (R3, R5, R12).** A release that predates the
  feature can rewrite a header and push a session; the machine record must
  outlive that, and such a session must read as unknown, not as wrong.
- **One writer per object.** Several machines push to the same bucket at
  once, and S3-compatible providers don't all offer conditional writes, so
  nothing should need two machines to update the same object.
- **Existing bucket readers keep working.** Every caller that lists the
  bucket's top level (`held_elsewhere`, the continues walk, prune) must
  ignore anything this design adds there.
- **Small blast radius.** The change should touch the dashboard read path,
  session creation and the tick's push, not `koto session list` or the
  continuity mechanics.

## Considered Options

### Decision 1: How a reading machine finds remote sessions and reads their state

Delegated to /decision.

The dashboard has to list every session other machines pushed to the bucket,
each with its current state, without downloading any event log (R21: at most
64 KiB of object data per remote row per refresh, and no object GET on a
refresh where nothing changed). It must reflect a change within 60 seconds
in the TUI with up to 200 workspace prefixes (R22), finish `--once` within
30 seconds for 200 prefixes and 2,000 sessions (R24), drop removed sessions
(R13), hide an import's migrated source (R11), and show sessions written by
releases that predate this feature as `unknown` rather than with a frozen
state (R12), all with list and get rights only (R17, R18). Today the only
object holding current state is the state file, which is the header plus
the whole log, and the only listing that crosses workspaces is
`held_elsewhere`'s root listing, filtered to 16-hex prefixes by
`is_workspace_prefix` (`src/session/cloud.rs:187`). Every existing session
delete lists `<home>/<id>/` and removes what it finds there, in every
release (`deletable_keys`, `cloud.rs:236-248`), and every state push already
records the ETag its PUT returned.

Workspace prefixes grow faster than sessions do: each is a hash of the
directory koto ran in, and tooling that creates a directory per worktree
creates a prefix per worktree. One heavy host ran 1,497 sessions in 663
directories over three weeks.

Key assumptions:

- S3-compatible endpoints honour a multi-character delimiter (`/ctx/`) on
  ListObjectsV2, rolling each session's `ctx/` tree into one common prefix.
  Only AWS documents this; a live probe against R2, AWS, MinIO and B2 gates
  the layout (Implementation Approach, phase 1). If R2 or AWS fails it, the
  layout falls back to the sibling summary below under the same reader
  rules. If only a minor endpoint fails, that endpoint lists each session
  directory with `/` instead, which is correct but costs more.
- Nothing bulky lives in a session directory outside `ctx/`. A unit test on
  the key builders guards it.
- Listing after a PUT is consistent on supported providers; where it isn't,
  a change shows one refresh late.
- The gap between the state PUT and the summary PUT is short next to one
  refresh.
- An always-open TUI at 200 prefixes making about 200 listings per refresh
  is an acceptable cost (about $45 a month in AWS list charges around the
  clock; a TUI open eight hours a day at 50 prefixes is about $11). If it
  isn't, the change beacon described under Alternatives can be added
  without changing the layout.

#### Chosen: A summary object in each session's own bucket directory, found by one listing per prefix

After every successful push of its own state, the writing machine PUTs
`<home prefix>/<id>/summary.json`: a small JSON object, capped well under
64 KiB (the intent is truncated to fit), holding the summary schema
version, `session_id`, name, home prefix, kind, parent and `created_at`;
the template name, `current_state`, `is_terminal`, `is_blocked`, `failed`,
intent, `salient_var`, `is_batch_task`, `is_request_leg` and
`last_event_at`; `last_seq`, `state_etag` (the ETag the state PUT just
returned) and the recorded machine name; and a `sources` list of
`{relation: imported | superseded, prefix, name, session_id}` built from the
session's own import and supersede records, at most 16 entries. The row
fields come from one
pure derivation shared with the local dashboard reader, so a remote row
equals what the producing machine's own `--once` prints. Internal sessions
(anchors, close sessions, `~` epoch branches) get no summary.

The reader lists the bucket's top level with delimiter `/`, keeps the
16-hex prefixes, then lists each `<prefix>/` with delimiter `/ctx/`, 64
requests in flight, following every page. Each session's top-level objects
(state file, `summary.json`, `migrated.json`, `version.json`,
`template.json`, `batch-open.json`) come back with ETag, LastModified and
Size, and its `ctx/` tree folds into one entry. A pure classifier then
decides each session's row: no state object means the session was removed
(drop it); a `migrated.json` in the session's own directory whose named
copy appears in the same scan, or a local session's import or supersede
record naming it, means a migrated or superseded source (hide it, and count
it); a `sources` entry in another remote row's
summary that names it by `session_id` only recedes it, since any bucket
writer can write such an entry (see Security Considerations);
a state object with no summary is a legacy session (show it as `unknown`,
idle time from the state's LastModified, unless its name marks it
internal); a summary is fetched only when its listed ETag changed (the
TUI) or on every run (`--once`), and reads as `unknown` if it's over
64 KiB by its listed Size, doesn't parse, has the wrong schema version, or
names a different session than its key. A summary whose `state_etag` no
longer matches the listed state file (an older release pushed the state
without refreshing the summary) reads as `unknown` once the state file's
LastModified is more than 60 seconds old, in the TUI and `--once` alike;
inside that window it's the ordinary gap between the two PUTs.

No delete path changes: cleanup, prune, `prune --migrated-from` and
relocate already remove everything under `<home>/<id>/`, in every release,
so the summary never outlives its session.

The scan is paced. `--once` lists every prefix and fetches every summary on
each run: 201 listings and 2,000 GETs at 200 prefixes and 2,000 sessions,
under two seconds at 64 in flight and 50 ms per request. The TUI's remote
refresh runs every 55 seconds with a budget of about 200 prefix listings:
at 200 prefixes or fewer every prefix is listed every refresh; above that,
a newly seen prefix and every prefix with a state change in the last 30
minutes are listed every refresh, and the rest of the budget rotates
through idle prefixes (each rechecked about every five minutes at 1,000
prefixes).

This is the only placement every release's existing delete already
removes, and the same listing that finds a summary also returns the state
file's ETag and the `migrated.json` marker, which is what lets the reader
refuse a stale summary and hide a migrated source even when an older
release ticked, deleted or imported the session. The accepted trade is a
list-heavy refresh: at 200 or more prefixes, an always-open TUI costs about
seven times what a change-beacon design would.

#### Alternatives Considered

**Scan every prefix and fetch changed state files by ETag.** No format
change, but current state needs the log, the init event and the template,
so a changed session costs up to its whole log (6.4 MB on one host), far
past 64 KiB. Rejected because it breaks R21; it survives only as the
treatment of legacy rows, which are shown as unknown without fetching the
log.

**A per-session summary under a top-level index prefix
(`koto-index/<prefix>/<id>.json`).** Cheapest to read (about two listings
per refresh). Rejected because the entry is a second record of each session
outside its directory: an older release's delete, a relocate, a hand
deletion or a lifecycle rule leaves an entry that looks live, and the
read-only reader can never remove it. It also needs entry deletes in five
code paths, some inside another machine's namespace, and covering legacy
sessions within 60 seconds needs the same per-prefix listings anyway.

**A per-machine index object.** Rejected: every tick rewrites one object
every koto process on the host shares, which needs a host-wide lock and a
read-merge-write; a changed machine costs the reader its whole index
(about 600 KB); its natural key is the hostname-derived `machine_id`, which
collides between hosts; and legacy sessions are invisible.

**A sibling summary at prefix level (`<prefix>/_<id>.summary.json`).**
Works with the plain `/` delimiter at the same listing count and no
existing lister sees it. Rejected as the primary because no release's
delete removes it: it needs tombstones and new deletes, leaves orphans,
and can't see stale summaries or older releases' imports without an extra
sweep. It's the fallback if the `/ctx/` probe fails on R2 or AWS.

**The in-directory summary read with one listing per session.** Correct,
but 2,201 listings per refresh at the PRD's scale. Kept only as the
per-endpoint fallback when an endpoint ignores the delimiter.

**A per-prefix change beacon on top of the chosen layout.** A root-level
`<prefix>.changed` object rewritten after each summary write, with the
reader re-listing only prefixes whose beacon moved plus a ten-minute
sweep, cuts the TUI's listing bill to about an eighth. Not adopted now: it
needs the PRD to let changes made by older releases take up to ten
minutes, it adds a second PUT per push on a key several hosts write, and it
does nothing for `--once`. A later release can add it without changing the
layout, since existing listers ignore root-level objects.

### Decision 2: Where the producing machine is recorded

Resolved inline.

R1 and R3 ask for a machine name recorded at creation that survives a
rebind, a push and pull, a resolve, and a header rewrite by any release,
including one that predates this feature; an import records the importing
machine on its copy and leaves the source alone; R4 keeps `origin`'s shape.
`StateFileHeader` round-trips through a struct with no catch-all field, so
an older release that rewrites a header drops any field it doesn't know.
Event payloads are read by `event_type` and their structs accept unknown
fields (`EventPayload`, `src/engine/types.rs:1228-1250`), and the log is
append-only: no release rewrites an existing event line, and import copies
event lines verbatim.

Key assumptions:

- No code path rewrites existing event lines (rewind, cancel and supersede
  all append). The implementing issue's tests confirm it.
- `machine.name` is a new key read by the same user-level resolver that
  reads `machine.id`.

#### Chosen: On the session's own events

`workflow_initialized` gains an optional `machine` field holding the machine
name at creation, written by `koto init`, a child spawn and an inline init
on every backend. `session_imported`, which import already appends with the
importing machine's id, gains an optional `machine_name` field. A
session's producing machine is the `machine_name` of its latest
`session_imported` event when it has one, else the `machine` of its
`workflow_initialized` event, else unknown. Both fields follow the existing
additive pattern (`#[serde(default, skip_serializing_if = "Option::is_none")]`,
like `spawn_entry`) and don't move `CURRENT_SCHEMA_VERSION`.

The name is resolved once per process: `KOTO_MACHINE_NAME` when set and
non-empty, else `machine.name` from the user-level configuration file, else
the hostname with any domain part removed. A valid name is 1 to 64
characters of `[A-Za-z0-9._-]`, and the name `unknown` is reserved for
sessions with no record. `koto config set --user machine.name` refuses
anything else; an invalid environment value is ignored with one
warning on stderr; a hostname is coerced (other characters become `-`, then
it's cut to 64, and a hostname of `unknown` becomes `unknown-host`).

#### Alternatives Considered

**A header field, inside or beside `origin`.** Rejected: a release that
predates this feature drops it the first time it rewrites the header, which
is exactly the case R3 names; inside `origin` it would also change
`origin`'s shape (R4), and import rebuilds `origin`.

**`version.json`'s `machine_id`.** Rejected: it's an eight-hex hash, not a
name; import overwrites it; it describes the last writer, not the producer;
and a local-backend session has no copy of it.

**Both a header field and an event field.** Rejected: two records of one
fact that disagree after an older release rewrites the header.

### Decision 3: How remote rows reach the dashboard

Resolved inline.

R23 says no bucket request runs on the thread that handles input and
rendering, and that a key press shows within 200 ms even when the endpoint
never answers. Today `refresh()` runs on the UI thread every 500 ms and,
under the cloud backend, already lists the current prefix there. R9 and R10
need rows to be distinct per session copy and a session with a local copy to
appear once. The TUI and `--once` share `refresh()` today, and the evidence
scenarios compare them.

Key assumptions:

- The dashboard's row identity and expansion state (`RowDescriptor` and the
  maps in `src/cli/dashboard_state.rs`) can take a composite key without
  changing how local-only rows sort or render.

#### Chosen: A background remote feed and a composite row key

The TUI starts one fetcher thread that owns every bucket request, runs the
Decision 1 scan on its 55-second clock, and hands each complete snapshot of
remote rows, with its failure status, to the UI thread over a channel.
`refresh()` merges the latest snapshot without waiting. `--once` runs the
same fetch routine once, synchronously, with an overall deadline, before it
prints. Rows are keyed by `RowKey { session_id, location }` where location
is `Local` or `Remote(workspace id)`; expansion, selection and parent
nesting use the key, and nesting only links rows with the same location.
Before the merge, remote rows whose `session_id` matches a local session
are dropped (legacy remote rows, which have no summary, match on the local
header's recorded home prefix and name).

#### Alternatives Considered

**Remote reads inline on a slower tick.** Rejected: the UI thread still
blocks for that tick's requests, up to the 10-second timeout each, which
fails R23.

**A separate remote pane or tab.** Rejected: it breaks the single attention
ordering the evidence scenario relies on (a remote blocked session in the
needs-you band) and doubles the list surface.

**Returning remote rows from `CloudBackend::list()`.** Rejected: `koto
session list` shares it, and the PRD keeps that command unchanged.

## Decision Outcome

The three decisions meet at the summary object. At creation a session
records its machine on its first event (Decision 2). Every push of its
state then publishes a small summary beside the state file, derived from
the log by the same function the local dashboard uses and carrying the
machine name and the ETag of the state it describes (Decision 1). A
reading machine finds those summaries with one listing per workspace
prefix, fetches only the ones that changed, and classifies every session
directory it sees, so removed sessions drop, migrated sources hide, and
sessions an older release wrote or last advanced read as unknown. All of
that runs on a background feed in the TUI and as one synchronous pass in
`--once`, and rows are keyed by session and location so a remote session
never collides with a local one (Decision 3).

Read against the PRD's evidence: on machine A a session stops at a gate;
its tick's push writes the state file and then `summary.json` with
`is_blocked: true`, the gate's state and A's machine name. On machine B,
the next feed pass (within 55 seconds) lists A's prefix, sees the summary's
ETag change, fetches it and places the row in the needs-you band with
`@<A>`; `koto dashboard --once` on B does the same pass at once and prints
the session's columns plus `machine` and `workspace`.

The questions the feature left open resolve here. Credentials: none are
added; the reader uses what koto already resolves and needs only list and
get. Discovery: one top-level listing plus one `/ctx/`-delimited listing
per workspace prefix. Stamping: on the `workflow_initialized` event at
production time, and on `session_imported` for an import. That differs
from today's `origin` in both what it records (a machine, where `origin`
records the anchor directory and store identity) and where it lives (an
append-only event, where `origin` is a header field import and rebind
rewrite). Text columns: `machine` and `workspace` appended as columns nine
and ten.

## Solution Architecture

### Overview

Two additions on the writing side (a machine stamp at creation and a
summary at every push) and one on the reading side (a remote feed the
dashboard merges), joined by one shared derivation of a session's row.
Nothing changes in `koto session list`, the continuity mechanics, the
delete paths or `origin`.

### Components

- **Machine name resolver** (`src/session/version.rs`, beside
  `get_or_create_machine_id`): `machine_name() -> MachineName` resolving
  `KOTO_MACHINE_NAME`, then `machine.name` from the user-level file, then the
  coerced short hostname; validation shared with `koto config set`
  (`src/config/`), which registers `machine.name` as a user-level key.
- **Event stamp** (`src/engine/types.rs`, `src/cli/init_child.rs`, the
  import path in `src/session/cloud.rs`): `WorkflowInitialized.machine` and
  `SessionImported.machine_name`, written at init, child spawn, inline init
  and import; `producing_machine(events) -> Option<String>` reads them back.
- **Row derivation** (new `src/session/row.rs`, lifted out of
  `read_session` in `src/cli/dashboard_data.rs:381-508`):
  `derive_row(header, events, template) -> SessionRow` (it needs the event
  stamp for the producing machine), a pure function
  returning current state, terminal/blocked/failed flags, intent, template
  name, salient variable, batch and request-leg flags, last event time,
  parent and producing machine. The local dashboard and the summary writer
  both call it.
- **Summary writer** (`src/session/summary.rs`, called from `put_state`
  after `record_synced`, from `put_state_if` and `put_state_over`, and once
  after the import's `push_staged`, which is changed to keep the state PUT's
  ETag): builds a `SessionSummary` from `derive_row` plus `last_seq`,
  `state_etag` and `sources`, applies the per-field caps, serializes it
  under the size cap, and PUTs `<home>/<id>/summary.json`. It derives the
  row from the events and template the push already holds in memory, so it
  never re-reads the log. A failed PUT warns like any push failure, never
  fails the command, and marks the session so the next flush writes the
  summary even if the state didn't change; the write honours
  `refuse_if_read_only`; internal sessions and `push_fence` write none.
- **Remote reader** (`src/session/remote_feed.rs`): `scan(budget, cache,
  locals) -> RemoteSnapshot`. Builds its own S3 client through the same
  constructor `create_bucket` uses (endpoint, region, path style, TLS and
  credentials resolved as for every other command), adding a 10-second
  request timeout and no write method. It reuses the top-level listing and
  16-hex filter `held_elsewhere` uses, lists each chosen prefix with
  delimiter `/ctx/` (falling back per endpoint), classifies each session
  directory, fetches changed summaries with the body read capped at 64 KiB
  plus one byte, and maps every failure to `unreachable`, `denied`,
  `timeout` or `throttled`. It holds its ETag cache in memory only, writes
  nothing to the bucket or the session store, and is never given the
  `CloudBackend`'s sync paths. The sanitizer lives here, so no unsanitized
  bucket string leaves the reader.
- **Dashboard integration** (`src/cli/dashboard_data.rs`,
  `dashboard_state.rs`, `dashboard_render.rs`, `dashboard.rs`): a
  `RemoteFeed` thread the TUI starts with its first frame and stops when it
  exits (dropping the channel ends the thread after its current request),
  which reads the current local set (`session_id`s plus each local
  session's home prefix and name) from a shared handle the UI thread
  updates every refresh; a synchronous call for `--once`; the
  `RowKey` composite key; the `@<machine>` / `@unknown` marker on remote
  rows; the Machine row and workspace id in the Session tab and the one-line
  notice on a remote row's other tabs; the status line for a failed or
  partial snapshot; the two appended `--once` columns; the `machine` field
  in `--detail`; and one sanitizer applied to every bucket string before it
  reaches a row.

### Key Interfaces

`summary.json` (schema 1):

```json
{
  "schema": 1,
  "session_id": "...", "name": "review", "home": "3f2a9c0d4b1e7a65",
  "kind": "workflow", "parent": null, "created_at": "...",
  "template_name": "...", "current_state": "...",
  "is_terminal": false, "is_blocked": true, "failed": false,
  "intent": "...", "salient_var": null,
  "is_batch_task": false, "is_request_leg": false,
  "last_event_at": "...", "last_seq": 42,
  "state_etag": "...", "machine": "build-vm",
  "sources": [{"relation": "imported", "prefix": "...", "name": "...", "session_id": "..."}]
}
```

A reader treats an unknown `schema` value, a body over 64 KiB, a parse
failure, or a `session_id`, `name` or `home` that disagrees with the
object's key as an unreadable row.

`koto dashboard --once` line: the eight existing tab-separated columns, then
`machine` (recorded name or empty) and `workspace` (16-hex prefix, empty on
the local backend). `koto dashboard --detail <name>` adds `"machine":
<string|null>`; `docs/reference/session-feed.md` documents both.

The reader's types:

- `RemoteRow { key: RowKey, row: SessionRow, workspace: String, status:
  Shown | Unknown(reason) | Receded | Hidden }`, every string sanitized.
- `RemoteSnapshot { rows: Vec<RemoteRow>, failure: Option<{class, host,
  failed_workspaces}>, unreadable: usize, hidden_sources: usize,
  truncated: bool, taken_at }`.
- `EtagCache`: `(workspace, session_id) -> (summary_etag, SessionRow)`, in
  memory, owned by the feed thread.

The classifier applies these rules to each session directory in order; the
first that matches decides:

| Order | Listing shows | Row |
|-------|---------------|-----|
| 1 | no state object | dropped (removed session) |
| 2 | internal name (`~`, `anchor-` prefix, `-closed` suffix) | dropped |
| 3 | `session_id` or (home, name) of a local session | dropped (shown as local) |
| 4 | `migrated.json` naming a copy present in this scan, or named by a local import or supersede record | hidden, counted |
| 5 | state object, no summary | `unknown`, counted unreadable |
| 6 | summary over 64 KiB, unparseable, wrong schema, key mismatch, or `sources` over 16 | `unknown`, counted unreadable |
| 7 | summary whose `state_etag` differs and state older than 60 s | `unknown`, counted unreadable |
| 8 | named in another remote summary's `sources` | receded |
| 9 | otherwise | shown from the summary |

Configuration: `KOTO_MACHINE_NAME` and the user-level key `machine.name`.

### Data Flow

1. `koto init` on A resolves the machine name and writes it on
   `workflow_initialized`.
2. Each tick on A appends events, and `flush_pending` pushes the state file
   once (as today); `put_state` records the returned ETag and then PUTs
   `summary.json` built by `derive_row`.
3. On B, the feed (or `--once`) lists the bucket's top level, then A's
   prefix with `/ctx/`; it sees the summary's new ETag, fetches it, and
   classifies the row.
4. The snapshot reaches the UI thread; `refresh()` drops remote rows that
   duplicate local sessions, merges the rest under `RowKey`, and the
   existing liveness and attention rules place them.
5. Cleanup or prune on A deletes the session directory, summary included;
   B's next listing shows no state object for it and the row drops.

## Implementation Approach

### Phase 1: Delimiter probe

Settle the layout before anything depends on it. Add a probe to the cloud
integration tests (which run against Cloudflare R2 in CI) that writes a
session-shaped directory with a `ctx/` tree and checks that a
`/ctx/`-delimited listing returns the top-level objects and one common
prefix. Run it by hand against AWS, MinIO and B2 and record the results in
the cloud sync guide. A failure on R2 or AWS switches the layout to the
sibling summary before phase 4 starts; that layout needs tombstones and new
deletes in cleanup, prune and relocate, so the plan gains an issue for them
and is re-checked before phase 4.

The writer (phases 2 to 4) can ship a release before the reader, so
summaries are already in the bucket when the first reader looks for them.

Deliverables:
- a probe test in the cloud integration suite
- recorded results per provider

### Phase 2: Machine name and event stamp

Deliverables:
- `machine_name()` resolver and `machine.name` user-level key with
  validation
- `WorkflowInitialized.machine`, `SessionImported.machine_name`, written on
  every creation path and on import
- `producing_machine()` and tests that a rebind, push and pull, resolve and
  an older release's header rewrite leave it unchanged

### Phase 3: Shared row derivation

Depends on phase 2, since the row carries the producing machine.

Deliverables:
- `derive_row` lifted out of `read_session`, with the local dashboard
  calling it and its output unchanged on the existing dashboard tests

### Phase 4: Summary writer

Depends on phases 1 to 3.

Deliverables:
- `SessionSummary` and the PUT after `put_state`, `put_state_if`,
  `put_state_over` and import (keeping the import's state ETag), with the
  per-field caps and the retry on the next flush
- tests that a tick issues exactly one extra PUT, that internal sessions
  write none, and that cleanup and prune remove the summary

### Phase 5: Remote reader

Depends on phases 1 and 4.

Deliverables:
- `remote_feed::scan` with the top-level and per-prefix listings, the
  delimiter fallback, the pure classifier, ETag-gated fetches, the pacing
  budget, the 10-second timeout and the failure classes
- fake-bucket tests for removal, migrated and superseded sources, legacy
  sessions, stale summaries, oversized and malformed summaries, partial
  failures, pagination and the request counts R21 and R24 bound
- the Security Considerations rules: corroborated hiding only, the
  `sources` cap, strict summary parsing, `machine` re-validation, timestamp
  clamping, the per-run ceilings with their truncation notice, and the
  sanitizer

### Phase 6: Dashboard integration

Depends on phases 2 and 5.

Deliverables:
- the TUI feed thread, `RowKey`, merge and dedup, marker, Session tab facts
  and status line
- the `--once` columns and `--detail` field
- the two evidence scenarios as integration tests against the fake bucket

### Phase 7: Documentation

Depends on phase 6.

Deliverables:
- the cloud sync guide (permissions for a read-only watcher, listing cost,
  the probe results, and the rewritten sentence about indexes), the
  dashboard guide, `docs/reference/session-feed.md`, and the koto-user
  skill's command reference

## Security Considerations

The bucket is one trust domain. Any machine holding write credentials for it
can write any object, so everything the reader takes from the bucket
(listings, summaries, machine names, states) is treated as untrusted input
for display, and nothing it reads is executed, followed as a path, or used
to decide a write. The dashboard is an awareness aid: a forged summary can
make a row show the wrong state or the wrong machine, and that residual risk
is accepted and stated in the cloud sync guide. What the design does
prevent is a bucket writer using the dashboard against the person reading
it.

**No silent hiding.** A summary's `sources` list can't hide another
session's row: a `sources` entry from a remote summary only recedes the row
it names, which stays reachable with `--all` and the `a` key, and `sources`
is capped at 16 entries (a summary with more is unreadable). Hiding needs a
`migrated.json` in the hidden session's own directory whose named copy is
present in the same scan, or a local import or supersede record. Every
hidden row is counted on the status line and in `--once`'s stderr note, so
hiding is never silent. This raises the bar without removing it: a writer
that plants both a marker and a copy can still hide a row, and that is part
of the accepted residual risk above.

**Strict parsing and size limits.** A summary is fetched only when its
listed Size is at most 64 KiB, and the read stops at 64 KiB plus one byte,
so an object swapped between the listing and the GET can't exceed the bound;
a body that reaches the limit is unreadable. Each field has its own cap
(name, template name, state and salient variable 256 bytes each, intent
1 KiB, machine 64), checked by the writer and again by the reader. The body
is parsed into a closed schema:
an unknown `schema` value, a missing required field, or a `session_id`,
`name` or `home` that disagrees with the object's key makes the row
unreadable rather than partly trusted. The `machine` value is re-validated
against the same 1-to-64-character `[A-Za-z0-9._-]` rule the writer uses,
with `unknown` reserved; a value that fails is shown as `unknown`, so a
missing record and a malformed one look the same and neither can pose as a
real machine named `unknown`. Timestamps later than the reader's
clock plus five minutes are clamped to the reader's clock, so a forged
future time can't pin a row at the top of a band.

**Terminal output.** One sanitizer handles every string that came from the
bucket before it reaches a row: it replaces C0 and C1 control characters,
DEL, U+2028, U+2029 and the bidirectional override and isolate characters
(U+202A to U+202E, U+2066 to U+2069), the zero-width and directional marks
(U+200B to U+200F, U+061C, U+FEFF) and the tag characters (U+E0000 to
U+E007F) with a space, which removes ESC and so
every CSI and OSC sequence, and removes tab and newline from `--once`
fields. Tests feed each of these through `--once` and the TUI's rendered
buffer. Remote rows offer no action: the TUI's keys on a remote row only
move the selection and open its Session tab, and no remote string reaches a
shell or a koto command line.

**Resource ceilings.** A hostile or simply huge bucket can't hang or exhaust
the reader. Every request times out after 10 seconds, the TUI's scan has
its per-refresh budget, and each run stops at 5,000 workspace prefixes, 100
listing pages per prefix and 20,000 sessions, showing a "listing
truncated" notice on the status line (and stderr for `--once`) when a
ceiling is reached. Local rows are never dropped by a ceiling; prefixes are
taken in order of their most recent state change as the listings report it,
so the sessions most likely to need the developer survive truncation. A
writer can still pad the bucket to push real sessions past a ceiling, or
forge blocked rows to crowd the needs-you band; both are visible (the
truncation notice, the machine marker on every forged row) and part of the
accepted risk. Request cost is bounded per run and, in the TUI, per
refresh; a long-running TUI's total cost is documented rather than
capped.

**Concurrent writers.** Two machines can push the same session when their
checkouts share a path (and so a prefix). The summary is last-writer-wins
like the state file; when the two disagree, the `state_etag` check reads
the row as `unknown`, so the failure is visible rather than wrong.

**Permission scope.** The reader needs `s3:ListBucket` and `s3:GetObject`
only and builds its own client that has no write path; a test runs the
dashboard against the fake bucket and asserts no PUT, DELETE or POST, and no
new directory in the session store. The writer's credentials and paths are
unchanged apart from one more PUT per push, into the session's own
directory, which the existing delete already covers. Credential resolution
doesn't change, and no setting comes from a project's files.

**Data exposure.** A summary publishes the session's name, template name,
state, intent, salient variable and machine name into the bucket. All of
them except the machine name are already in the state file beside it, so
the summary gives no new access to anyone who can read the bucket. What
changes is display: the dashboard now gathers every workspace's intents and
states on one screen, which matters on a shared or recorded screen, and the
dashboard guide says so. The machine name defaults to the short hostname,
which is new in the bucket. The PRD chose the hostname default so a
developer on several machines needs no setup; the cloud sync guide states
it, and a developer who doesn't want the hostname published sets
`machine.name` or `KOTO_MACHINE_NAME` to something else.

**Existing listers.** The summary sits inside the session's directory, not
at the top level, so `held_elsewhere`, the continues walk and prune, which
read only 16-hex top-level prefixes, see nothing new. `koto session list`
reads only common prefixes under the current prefix and is unaffected.

## Consequences

### Positive

- A developer sees every machine's sessions, with state and machine, from
  any machine, with no new service or credential.
- Removed sessions, migrated sources and sessions advanced by older
  releases are handled correctly in a bucket that mixes releases, because
  the reader checks each summary against the state file it sits beside.
- No delete, prune, relocate or root-level lister changes, and `koto
  session list` is untouched.
- Local and remote rows come from one derivation, so they can't drift.

### Negative

- Every push makes one more PUT.
- Refreshes are list-heavy: about 200 listings per TUI refresh at 200 or
  more prefixes, and a `--once` loop pays one listing per prefix plus one GET
  per session on every run.
- The layout depends on a multi-character delimiter only AWS documents.
- Above 200 prefixes, a change in a long-idle workspace can take one
  rotation (about five minutes at 1,000 prefixes) to show.
- Listing pages fill at about 200 sessions per prefix instead of 1,000,
  because each session contributes several top-level objects.
- The machine name is whatever the producer wrote; a bucket writer can
  claim any name.

### Mitigations

- The probe in phase 1 gates the layout, with the sibling summary as a
  ready fallback and a per-endpoint fallback for minor providers.
- The guides state the listing cost for an always-open TUI and a `--once`
  loop, and the change beacon remains an additive valve a later release
  can ship without a layout change.
- The rotation bound is documented beside the 200-prefix figure the PRD
  states its bounds for.
- Every bucket string is sanitized and size-checked before display.
