---
schema: design/v1
status: Proposed
problem: |
  No read-only koto verb prints a session's gate evaluations. The session's
  event log records each one as a `gate_evaluated` event (gate, outcome,
  time, findings with their rule ids) and the run journal records them too,
  but both are files. A program that counts how often the same gate fails
  on the same rule across sessions has to parse koto's state files itself,
  and has no answer at all for a session in another workspace, since
  `--workspace` serves only the read verbs it lists (#688).
decision: |
  Add `koto evaluations list <session>`, which prints one JSON object: the
  session's name, id, current state and template (name, hash, source file)
  once at the top, then every `gate_evaluated` event in its log, oldest
  first, as a record with seq, timestamp, state, gate, outcome, reason,
  attempt numbers, the error rule ids in `rule_counts`, the findings reduced
  to rule id and level, both truncation flags and the poll record. It reads
  only the event log, one session per call, joins the `--workspace`
  allowlist, and under the cloud backend uses a new backend read that never
  flushes or takes the batch lock. It writes nothing.
rationale: |
  The event log is the record every host and backend agree on and the one
  `--workspace` already fetches through its read-only reader, so the verb
  needs no new storage and one more arm in the allowlist. The run journal
  is per host, lossy, and being retired, so a verb built on it would go
  empty. A `<noun> list <session>` verb matches the read verbs callers
  know, keeps its own output contract, and avoids making every event
  payload public as a raw dump would. The read that never flushes keeps a
  sweep from making running ticks wait. The cost is that sessions koto has
  already removed can't be read, which the design states.
upstream: docs/briefs/BRIEF-gate-evaluations-read.md
user_visible_surface: true
---

# DESIGN: A read-only verb for a session's gate evaluations

## Status

Proposed

## Context and Problem Statement

koto evaluates a state's gates on every `koto next`, and each evaluation is
recorded: the session's event log gets a `gate_evaluated` event carrying the
state, the gate's name, the outcome, the time and, for a command gate, the
findings the check printed, each with its `rule_id`. The run journal on the
host that ticked the session gets the same evaluation as `check_evaluated`
and `check_rule` records. Neither is reachable through a verb. The read-only
verbs show other things: `koto status` prints the current state and
directive, `koto dashboard --detail` names the template but no evaluation,
and `koto overrides list`, `koto decisions list` and `koto context list`
print what their names say (#688).

So a program that wants to know which gates failed in a session, and on
which rules, has to parse koto's files itself. The case that raised this is
a sweep that counts how often the same gate fails on the same rule across
many sessions, to find rules that keep firing. For each evaluation it needs
the gate's name, the rule id of each finding, the template the session runs
and the time. Reading `gate_evaluated` out of a state file ties that program
to koto's on-disk layout and to the event schema, and leaves it with no
answer at all for a session in another workspace, since `--workspace` only
serves the verbs it lists: `status`, `context get`, `context exists`,
`context list`, `decisions list`, `decisions show` and `overrides list`.

This design decides how koto exposes a session's gate evaluations to a
reader: where the answer comes from, what one call covers, what each record
holds and how the verb behaves for a session in another workspace.

## Decision Drivers

- **Read without parsing koto's files.** The reader gets the records from a
  koto verb, in a documented JSON shape, and needs no knowledge of the state
  file layout, the event schema or the run journal's line format.
- **Enough per evaluation to count repeats.** Each record carries the gate
  name, the outcome, the rule id of each finding, the template and the
  evaluation time, so a caller can group by (template, gate, rule) and count
  without a second lookup.
- **Same answer on every host and backend.** A cloud session read on any host
  that can reach it, and a local session, answer from the same records,
  as `koto context history` does (#685).
- **Reaches another workspace where the other read verbs do.** The answer for
  a session in another workspace follows the `--workspace` rules the read
  verbs already follow, rather than inventing a second path into the bucket.
- **Reads change nothing.** No event in the session, no `context_read`, no
  run journal record, no lock taken that a running `koto next` would wait on.
- **Honest about what the log kept.** The log caps each evaluation's findings
  and marks the cut; the record says when a rule list is incomplete rather
  than presenting a truncated list as the whole.
- **Small surface, consistent with koto's verbs.** It follows the shape the
  existing `<noun> list <session>` read verbs use (JSON on stdout, exit 0 /
  2 / 3) and adds no new storage.

## Considered Options

### Decision 1: Which record the read answers from, and what one call covers

A session's gate evaluations are written twice. The session's event log gets
a `gate_evaluated` event per evaluation with the state, gate, outcome,
timestamp, attempt numbers, up to 50 findings and, on a failed check, up to
50 error rule ids in `rule_counts`, which koto builds from every finding
before the findings cap applies. The run journal on the host that ticked the
session gets `check_evaluated` and `check_rule` records, with the template
only on the session's `session_started` line. The journal outlives sessions
koto removes, which matters here: a lone session that reaches a success
terminal loses its directory, log included, on that same tick.

The journal gives nothing per evaluation that the log lacks. Its
`check_rule` records copy `rule_counts` with the same cap, rewrite rule ids
outside the name shape to `sha256:` digests, and are skipped when the
journal's lock can't be taken within five seconds. It's per host, never
synced or imported, so a cloud session continued on a second host has its
checks split across two journals. And it's being retired: the event sink
design (`DESIGN-event-sink.md`) lets each host turn the journal off and
koto#545 removes it in a later release.

On reach, `--workspace` already serves a verb that reads the log: `koto
overrides list` calls `read_events` through the backend the
`WorkspaceReader` hands it. `koto context history` refused `--workspace` as
a scope choice, not because of a hazard (#685). No listing of another
workspace's sessions exists yet (koto#343).

Key assumptions:

- The run journal is removed as koto#545 plans. If that is withdrawn and the
  journal becomes a supported record, a journal fallback for removed
  sessions is worth a second look.
- The sweep counts error-level rules, which `rule_counts` holds. Warnings
  and a passing check's findings exist only in the capped `findings` list.
- One call per session, with sessions enumerated by `koto workflows` or
  `koto session list`, is fast enough for a sweep. A store-wide mode can be
  added later without changing the per-session output.

#### Chosen: The session's event log, one session per call, served under `--workspace`

The verb answers from the session's event log and nothing else. It reads
the log through the session backend, takes every `gate_evaluated` event as
one record, and takes the template name and hash from the session header.
It never reads the run journal.

One call covers one named session, the shape of `overrides list <session>`
and `decisions list <session>`. The verb joins the `--workspace` allowlist
in `src/cli/workspace_read.rs`, so `--workspace <id>` reads a session in
another workspace, or one moved by `koto session import`, through the
existing read-only `WorkspaceReader`.

The log is the record the journal is derived from, it's the same record on
every host and backend, and it's already reachable through `--workspace` by
the path `overrides list` uses. That meets the same-answer and
reaches-another-workspace drivers with no new storage. A verb that read the
journal would print an empty answer with exit 0 on any host that turned the
journal off, and a sweep would take that as "no failures".

#### Alternatives Considered

- **Run journal, host-wide report.** A verb in the style of `koto decider`
  that reads this host's `_run_journal.jsonl` and joins check records to
  session records, for every session the host ever ticked, removed ones
  included. Rejected because the journal is being retired, so the verb would
  go empty and then have to be deleted. It's also per host, drops records
  under lock contention, rewrites some rule ids, splits a cloud session
  across hosts and can't serve `--workspace`. The host-wide shape wasn't the
  problem; the source was.
- **Event log first, journal for removed sessions.** The chosen verb, plus a
  fallback to this host's journal lines when the session is gone, marked
  `source: journal`. Rejected for the same retirement reason, and because
  the fallback answer depends on which host runs it, carries fewer fields
  than the log answer, and would make koto the first reader of a file it
  has only ever written.
- **Event log with a store-wide mode.** The chosen verb plus an `--all` form
  that streams every session in this store. Deferred rather than adopted. A
  form that reads only local copies can disagree with the per-session form
  for a cloud session whose newest events are on another host; a form that
  pulls each session does what the caller's loop does and saves only process
  starts; and `--workspace --all` needs koto#343. It can be added later
  around the same per-session records.

### Decision 2: The verb, its record and how it reads a cloud session

With the source and the reach settled, what's left is how a caller asks and
what it gets back. koto's read verbs give the pattern a caller expects:
`koto overrides list <session>` prints `{"state", "overrides": {"count",
"items"}}`, where `state` is the session's current state and each item
carries its own state and gate; `koto decisions list` uses the same
envelope and adds filters; `koto context history` exits 2 for a missing
session or a cloud session this host has no copy of (`no_local_copy`) and 3
for a log it can't read.

Two facts about the log shape the record. A rewind appends a `rewound`
event and removes nothing, so evaluations from before a rewind stay in the
log, and `overrides list` already lists every epoch. Gate names are unique
within a state but not across states, so a gate is named by (state, gate).
Pending evaluations, from a polling gate that hasn't settled, carry no
attempt stamp and no `rule_counts`.

The cloud read needs care. `CloudBackend::read_events` pulls before it
reads, and when this host has an open write batch for the session the pull
flushes it: it takes the batch lock and pushes the batch to the bucket
while holding it. A running `koto next` takes the same lock for each write
it defers into its batch, so a reader that flushes makes that tick wait on
network round trips and sends its batch up early (koto#548). `status`,
`overrides list` and `context history` all behave this way today. The served
reads also pass through `locate_or_serve`, which can PUT a continues
back-reference. A backend opened read-only doesn't avoid the lock either.

Key assumptions:

- While a batch is open on this host, the local copy holds every event the
  bucket has plus the batch's own: a flush leaves the local copy equal to
  the bucket's, and if another host had pushed in between, the flush would
  refuse as a conflict anyway.
- The ordinary pull replaces this host's local file with a newer bucket
  copy only when that copy holds every local event (koto#477), but a tick
  can append between that comparison and the replace. A read that never
  replaces the file has no such window.
- The sweep groups by `template.name`, falls back to `template.hash` when no
  name was recorded, and keys a gate on (state, gate).
- A sweep filters by outcome, state or gate itself, so filter flags can
  wait.

#### Chosen: `koto evaluations list <session>`, template once at the top, a read that never flushes

A new noun, `koto evaluations list <session>`, with the help text "Gate
evaluation retrieval (reads only)". koto's read nouns name the record kind
they list (`decisions`, `overrides`), and this one lists `gate_evaluated`
events. It takes no flags of its own and joins the `--workspace` allowlist.

The response is one JSON object: the session's name, id and current state,
the template once (name, hash, source file), and the evaluations as a
`count` and `items` envelope, one item per `gate_evaluated` event, oldest
first, every key always present. The full shape is under Solution
Architecture. Each item carries the event's `seq` and `timestamp`, `state`
and `gate`, `outcome`, `reason`, both attempt numbers, `rule_counts` with
its cut flag, the findings reduced to `{rule_id, level}` with their cut
flag, and the poll record. Messages, paths, the gate's `output`, the stream
copies, `duration_ms` and `rules_delivered` stay out.

The template goes at the top because it belongs to the session, not to the
evaluation, and one call covers one session, so the caller holds it beside
every item with no second lookup. That's where `overrides list` and
`decisions list` put session-level facts too.

Under the cloud backend the read never flushes, never clears an unopened
batch and never takes the batch lock. If this host has the session's batch
open, or the command runs inside a tick on it, the read takes the local
copy as it is, which is the newest copy there is. Otherwise it probes the
bucket and, when the bucket's copy moved, fetches it into memory and
answers from it if it holds every local event, without replacing this
host's file; a copy that diverges from the local log refuses, as the pull
does. It skips the back-reference PUT, appends no event, logs no
`context_read`, writes no journal record and leaves the local file as it
found it.

#### Alternatives Considered

- **Template repeated on every item.** The chosen verb with `template` on
  each record, so a flattened stream needs no join. Rejected because the
  template is fixed for the session's life, so a copy on each item is
  redundant and suggests a value that could vary, and a caller who wants
  flat rows gets them with one `jq` expression over the top-level copy.
- **A flag on `koto status`.** `koto status <session> --gates` would add the
  evaluations to the current-state view. Rejected because status has its
  own contract (the removed-session answer, the template hash mismatch
  report, the directive text), a flag would make its output shape depend on
  the flag, and the read would inherit status's flush and back-reference
  write.
- **An extension of `koto dashboard <session> --detail`.** Rejected because
  the dashboard reads state files by path, isn't in the `--workspace`
  allowlist, and its detail output is a view for people rather than a
  contract a program can rely on.
- **A generic event dump.** `koto session events <session> --type
  gate_evaluated` printing raw payloads. Rejected because it makes every
  event's payload a public CLI contract at once, prints captured stdout,
  stderr and gate outputs through `--workspace`, leaves the template off the
  records, and still needs the read that never flushes. Whether koto should
  have such a verb is a larger, separate question.
- **The noun `gates`.** `koto gates list <session>`. Rejected because
  `gates` names the template's gate definitions, and a reader would expect
  it to list the gates a state declares rather than the times they ran.
- **The cloud read the sibling verbs use.** Inherit `read_events`, flush
  included, as `status` and `overrides list` do. Rejected because a reader
  would take the batch lock a running tick's writes wait on and push the
  tick's batch early. The sibling verbs mostly read the session the caller
  is driving, between its own ticks; a sweep reads every session in the
  store, including ones other agents are ticking right now, so the flush would land on
  exactly the sessions it hurts and undo the one-push-per-tick batching
  koto#548 introduced. A sweep run often enough to be useful would turn
  every read into a cost on someone else's tick.
- **The local copy only.** Read `read_events_local` and never pull.
  Rejected because a cloud session continued on another host would be
  answered from a stale copy, and two hosts would give two answers.

### Decision 3: Which evaluations the list holds

Writing the architecture surfaced four smaller choices about what goes in
the list. Each could reasonably have gone the other way.

#### Chosen: every `gate_evaluated` event, pending and pre-rewind included, `__action__` left out, a removed session as not found

The list holds every gate evaluation in the log: pending ones, because
they're in the log and a reader drops them with one filter, and ones from
before a rewind, because a rewind doesn't undo the fact that the gate ran
and failed, which is what a repeat count counts. `overrides list` lists
every epoch for the same reason. Default-action checks are left out: the log
records them as `default_action_executed` events, not `gate_evaluated`, and
`koto disputes record` addresses them by the reserved name `__action__`, a
name no template declares as a gate. Mixing them in would put that name into
a count keyed on gate names. A session koto removed answers exit 2 with the
not-found error every read verb gives, since the verb has nothing to read.

#### Alternatives Considered

- **Settled evaluations of the current epoch only.** The view `disputes
  record` and `overrides record` take, where only the latest settled
  evaluation matters. Rejected because a sweep counting repeats wants every
  time the gate failed, and a rewind would otherwise hide failures that
  happened.
- **Default-action checks included, marked by kind.** Rejected for now:
  they're a different record with a different meaning, and can get their
  own flag or noun if someone needs them.
- **A typed refusal for a removed session.** A `session_removed` code read
  from the session's finished record. Rejected for this change because it
  needs a read beyond the log; it can be added later without breaking a
  caller that already handles exit 2.

## Decision Outcome

koto gets one new read verb, `koto evaluations list <session>`, that prints
every gate evaluation a session's event log holds, oldest first, with the
session's template once at the top. A sweep that counts repeated failures
loops it over the sessions `koto workflows` or `koto session list` names and
groups the items by template name, state, gate and the rule ids in
`rule_counts`, which hold every error rule a failed check reported (up to 50
per evaluation, with a flag past that), even when the findings list was
cut. It never opens a state file or the run journal.

The two decisions fit because the log is both the record every host and
backend agree on and the record `--workspace` already knows how to fetch.
The verb answers a session in another workspace, or one imported elsewhere,
the way `overrides list` does: through the read-only `WorkspaceReader`, by
adding one arm to the `--workspace` allowlist. The one place it departs from
its siblings is the cloud read, which never flushes or locks, so a sweep can
run against sessions that are mid-tick without slowing them.

What the verb can't see is a session koto has already removed. A lone
session (no link, no lineage) that ends in success is removed on its final
tick unless it was ticked with `--no-cleanup`, so its evaluations go with
it. The sweep counts what the store keeps: failure terminals, pinned
sessions, and sessions whose hierarchy the retention rule still keeps
(a live member, or less than `retention.keep` since the hierarchy's
release). The records of removed sessions belong to the event sink's
downstream store, not to this verb.

## Solution Architecture

### Overview

`koto evaluations list <session>` resolves the session the way the other
read verbs do, reads its header and events through a backend read that
never flushes, keeps the `gate_evaluated` events, projects each into a
fixed record, and prints one JSON object. With `--workspace <id>` the same
handler runs against the backend a `WorkspaceReader` builds over the other
workspace's prefix. Nothing is written anywhere: not to the session, not to
this host's store, not to the bucket, not to the run journal.

### Components

- **`src/cli/evaluations.rs`** (new). The `EvaluationsSubcommand` enum with
  one variant, `List { name }`; `handle_list(backend, name)`, which reads,
  projects and prints; and the pure projection `evaluation_items(&[Event])
  -> Vec<EvaluationItem>`, unit-tested on its own. The projection bounds
  each string it emits and holds the findings and rule lists to the
  writer's caps, since a log read through `--workspace` is untrusted input
  (see Security Considerations).
- **`src/cli/mod.rs`**. A `Command::Evaluations { subcommand }` variant with
  the help text "Gate evaluation retrieval (reads only)", and its dispatch
  arm. The arm calls `locate_or_serve`, which gains a parameter that skips
  its `ensure_backref` call, so the read sends no back-reference PUT and
  the locate and migrated-source logic isn't copied. The current state
  comes from `derive_machine_state`, as `overrides list` derives it.
- **`src/session/mod.rs`**. A new additive method on `SessionBackend`,
  `read_events_unflushed(&self, id) -> Result<(StateFileHeader,
  Vec<Event>)>`, whose default is `read_events`. The local backend keeps
  the default: it has no batch to flush. The `Backend` enum, which
  forwards each trait method to its local or cloud backend by hand, gains
  the forwarding arm; without it every caller holding a `Backend`,
  `WorkspaceReader`'s included, would fall back to the flushing default
  with nothing to say so.
- **`src/session/cloud.rs`**. `CloudBackend` overrides it: run
  `check_not_migrated`; if this host's batch journal for the session is open,
  an opening file is there, or the process runs inside a tick on the
  session, read the local copy (`read_events_local`). Otherwise probe the
  bucket as `sync_pull_state` does, with its home, session-id and
  cross-import checks; when the bucket's copy moved, fetch it into memory,
  compare it with the local log (`state_log::compare`), and parse the copy
  that holds every event. A copy that diverges from the local log refuses,
  as the pull does. It never calls `try_clear_unopened_batch` or
  `flush_pending`, never takes the batch lock, and never writes the local
  file or its sync stamp.
- **`src/cli/workspace_read.rs`**. `read_verb` gains
  `Command::Evaluations { subcommand: List { name } }` as `"evaluations
  list"`; `serve` gains its dispatch arm; the refusal message that lists the
  verbs `--workspace` accepts names it.

The verb and the cloud backend both stay in the `koto` crate. If the shared
session reader (`DESIGN-koto-shared-session-reader.md`) moves the event
types into `koto-session-reader` first, the projection reads them at the
paths koto re-exports, and nothing here changes.

Neighbouring code this leaves alone:

- `koto context history` (#685) and the other read verbs keep
  `read_events` and its flush. Moving them to `read_events_unflushed` is a
  separate change with its own tests.
- koto#657 (a read verb's flush pushes a run's unpushed writes and leaves
  no event for that push) is about the flushing read. The new read never
  flushes, so it neither fixes nor changes that behaviour, and #657's fix
  doesn't change this read.
- A session whose bucket copy carries a supersede fence is read the way
  `koto status` reads it; the new read never locks or replaces the local
  file, so it doesn't touch the fence.
- The `locate_or_serve` parameter only skips `ensure_backref` on a located
  session. It changes nothing about how a session name is looked up, so
  the path for a name that isn't there stays as it is.

### Key Interfaces

The command:

```
koto evaluations list <session>
koto --workspace <id> evaluations list <session>
```

The response, one JSON object on stdout:

```json
{
  "session": "s1",
  "session_id": "6f1c...",
  "state": "check",
  "template": {"name": "doc-gate-probe", "hash": "a3f5...", "source_file": "t.md"},
  "evaluations": {
    "count": 1,
    "items": [
      {
        "seq": 3,
        "timestamp": "2026-11-12T22:11:40.123Z",
        "state": "check",
        "gate": "doc",
        "outcome": "failed",
        "reason": "rule_failed",
        "attempt": 1,
        "visit_attempt": 1,
        "rule_counts": {"prd/ac-numbered": {"visit": 1, "session": 1}},
        "rule_counts_truncated": false,
        "findings": [{"rule_id": "prd/ac-numbered", "level": "error"}],
        "findings_truncated": false,
        "poll": null
      }
    ]
  }
}
```

Top level: `session` is the name as given; `session_id` is the header's id,
`null` on a log written before ids existed; `state` is the session's
current state; `template.name` is the header's `template_name` (`null` when
not recorded), `template.hash` its `template_hash` (always present), and
`template.source_file` its source file name (`null` for a template read
from stdin or an older log).

Per item: `seq` and `timestamp` come from the event. `state` and `gate`
together name the gate. `outcome` is as logged (`passed`, `failed`,
`timed_out`, `error`, `pending`; the vocabulary is open, so a reader
tolerates others). `reason`, `attempt` and `visit_attempt` are `null` when
the event has none (a pass, a pending evaluation, an older log).
`rule_counts` is the event's map from error rule id to its visit and session
counts, `null` when the event carries none, and `rule_counts_truncated` says
whether rules past the cap were dropped. `findings` is one `{rule_id,
level}` per logged finding, in the log's order, `[]` when there are none,
and `findings_truncated` says whether the log cut the list. `poll` is the
polling record (`status`, `evaluations`, `since`, `elapsed_secs`) or `null`.
Every key is always present. Items are oldest first by `seq` and include
evaluations from before any rewind. Default-action checks, logged as
`default_action_executed` events, aren't gate evaluations and aren't listed.

Exit codes:

| Exit | When |
|------|------|
| 0 | The object. A session with no evaluations prints `count: 0` and `items: []`. A session imported elsewhere is answered from the copy it left in the bucket. |
| 2 | A caller problem, as `{"error", "command": "evaluations list", "code"?}`: a session that doesn't exist (`workflow '<name>' not found`), including one koto removed; a cloud session this host has no copy of (`code: "no_local_copy"`, naming `koto session resolve <name> --keep remote`); a session bound to another directory (the existing unlocated refusal). Under `--workspace`, the reader's own refusals keep their codes and exits. |
| 3 | The log exists but can't be read. |

### Data Flow

1. The CLI parses `evaluations list <session>`. With `--workspace`,
   `workspace_read::run` checks the verb against `read_verb`, opens a
   `WorkspaceReader` on the named workspace and hands its backend to the
   same handler; its scratch directory goes when the command exits.
2. The handler refuses an unlocated session, and serves a migrated source
   from its bucket copy, both as the other read verbs do, minus the
   back-reference.
3. `read_events_unflushed` returns the header and events. Under the cloud
   backend, an open batch on this host means the local copy is read as it
   is; otherwise the bucket's copy is fetched into memory if it moved, with
   no lock, and read in place of the local copy when it holds every local
   event. The local file isn't touched either way.
4. `evaluation_items` keeps the `GateEvaluated` events and projects each
   to an item; the handler adds the header fields and the current state,
   derived from the same events.
5. The object prints to stdout and the command exits 0.

## Implementation Approach

### Phase 1: A cloud read that never flushes

Add `read_events_unflushed` to `SessionBackend` with its default, and the
`CloudBackend` override. This goes first because it's the one piece with
locking behaviour, and the verb shouldn't ship on the flushing read.

Deliverables:
- `src/session/mod.rs` (the trait method and the `Backend` forwarding
  arm), `src/session/cloud.rs`.
- The tests below run through a `Backend`, and one through the backend a
  `WorkspaceReader` hands out, so a missing forwarding arm fails them.
- Cloud tests against the in-process fake S3: with a batch open on this
  host, the read returns the batch's events, the batch stays open and
  unflushed, and the fake S3 sees no write; with no batch open, a newer
  bucket copy is read and the local file and its sync stamp are
  byte-identical afterwards; a local append made after the probe is never
  lost; a bucket copy that diverges from the local log refuses; the batch
  lock is never taken.

### Phase 2: The verb

Add the `evaluations` noun, the projection and the handler.

Deliverables:
- `src/cli/evaluations.rs`, `src/cli/mod.rs`.
- Unit tests on `evaluation_items`: a failed command gate with findings
  and `rule_counts`, a pass, a pending poll, a truncated findings list, a
  rewind with evaluations on both sides, two states with a gate of the same
  name, a `default_action_executed` check left out, an empty log, and a
  hostile event with an oversized rule id, an oversized poll status and
  more findings than the cap, which comes out bounded with its truncation
  flags set, and one whose `seq` or counts aren't unsigned integers, which
  exits 3. A bucket copy over `MAX_STATE_BYTES` exits 3 without parsing.
- `tests/evaluations_list_test.rs`: #688's one-state probe template
  gives one failed item with `prd/ac-numbered` and the template name; a
  missing session exits 2; the log is byte-identical after the read; the
  cloud backend gives the same items as the local one, and a second host
  that takes a copy gets the same response.

### Phase 3: `--workspace`

Add the verb to `read_verb`, its arm in `serve`, and its name in the
refusal text.

Deliverables:
- `src/cli/workspace_read.rs`.
- `tests/workspace_read_test.rs`: the verb reads another workspace's
  session and a migrated source, writes nothing to either prefix, and
  removes its scratch directory; the walk over every verb that holds the
  allowlist fail-closed passes with the new arm.

### Phase 4: Documentation

Deliverables:
- `docs/guides/cli-usage.md`: an `evaluations list` section with the
  response, the exit codes and what the verb can't see (removed sessions),
  and the verb added to the `--workspace` lists.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md`:
  the verb.

## Security Considerations

The verb reads and prints. It writes nothing, runs no command, opens no new
network endpoint and adds no dependency, so supply chain trust doesn't
change. What needs care follows from where its input comes from and what it
prints.

**Input another party may control.** For a session in the caller's own
store the log is a file koto wrote. Under `--workspace`, and for a session
imported elsewhere, the log is the bucket copy under another workspace's
prefix, and whoever can write that prefix controls its bytes. koto's write
path bounds rule ids at 128 bytes, caps findings and rule counts at 50 per
evaluation and redacts known credentials, but none of that is applied again
on read. So the read bounds what it takes in and what it prints. A bucket
copy larger than `MAX_STATE_BYTES` (16 MiB, the cap koto already puts on a
fetched state file) is refused with exit 3 before it's parsed, which also
bounds how many items one response can hold. Each string the projection
emits (state, gate, outcome, reason, rule id, template fields, the poll
record's status and since, the timestamp) is cut to a fixed length; `seq`,
the attempt numbers, the `rule_counts` values and the poll record's counts
are parsed as unsigned integers, and an event whose fields don't parse
counts as an unreadable log; the findings and rule lists are held to the
writer's caps with the truncation flags set when the read cuts them. A log
that can't be parsed exits 3 with a message naming the problem and the event's
position, never a slice of the raw line. The output is one JSON document
serialized by serde, so quotes and control characters in a hostile value
are escaped and can't change the document's shape. Callers still treat
every string as data: a sweep doesn't pass a rule id or gate name unquoted
to a shell, a query, a path or a model prompt, and tolerates outcome and
reason values it doesn't know. The CLI guide says so.

**What it can reach.** `--workspace` stays fail-closed: the verb is added
to the allowlist as one more read, and every other verb keeps refusing with
`workspace_read_only` before anything is read. The `WorkspaceReader` is
read-only and removes its scratch directory however the command exits. The
verb grants nothing new: a caller who can point it at another workspace
already holds bucket credentials that let `koto context get` and `koto
overrides list` read that workspace's sessions, and this output is a subset
of what those credentials expose.

**The read that never flushes.** The new backend read drops the flush, the
unopened-batch cleanup, the batch lock and the back-reference PUT, which
removes writes a reader had no reason to make. It also never replaces this
host's local file: when the bucket's copy moved, the read fetches it into
memory and answers from it only if it holds every local event. An append a
tick makes while the read runs can't be lost, because the read never writes
the file the tick appends to. Phase 1's cloud tests cover an append made
after the probe.

**What it prints.** The session name and id, current state, template name,
hash and source file name, and per evaluation the seq, time, state, gate,
outcome, reason, attempt numbers, error rule ids with counts, findings
reduced to rule id and level, the truncation flags and the poll record. It
leaves out finding messages, paths and rule references, the gate's output,
the captured stdout and stderr, and the duration: the fields that carry
file contents, repository paths and tool output. Leaving them out means the
verb doesn't depend on redaction being complete. Rule ids, gate and state
names and template names are identifiers that check and template authors
choose; they show what a workspace runs and which rules keep firing, which
`koto status` and `koto overrides list` already show. Check authors should
keep per-instance data, such as names taken from the reviewed content, out
of rule ids, because redaction masks only values koto knows are secret. The
template source file is the header's bare file name, never a directory or
the compiled cache path.

**Residual risk.** Output read through `--workspace` is only as trustworthy
as the bucket prefix it comes from; the verb doesn't authenticate the log's
writer, so counts drawn from another workspace's or an imported session can
be fabricated by whoever can write that prefix, and a sweep shouldn't treat
them as authenticated. When gates ran and how often they failed is visible to anyone with
the workspace's bucket access, as it already is through `koto status` and
`koto overrides list`.

## Consequences

### Positive

- A program reads a session's gate evaluations through a documented verb
  and never opens a state file or the run journal, so koto can change
  either layout without breaking it.
- The answer is the same on every host and backend, and reaches another
  workspace through the path the read verbs already share.
- A sweep can run against sessions mid-tick without making their ticks wait
  or pushing their batches early.
- `read_events_unflushed` is there for `status`, `overrides list` and
  `context history` to adopt later if their flush becomes a problem.

### Negative

- The verb sees only sessions the store keeps. Lone sessions that ended in
  success are gone, so repeat counts lean toward failed, pinned and
  hierarchy-kept runs, and a rule that fired and was then fixed in a lone
  run isn't counted.
- The item fields become a public contract from the first release: a field
  can be added, not renamed.
- A sweep makes one process start per session, and there's no listing of
  another workspace's sessions yet (koto#343), so a cross-workspace sweep
  needs the names from elsewhere.
- While this host has a session's batch open, the read shows this host's
  copy; a push from another host in that window isn't seen until the batch
  closes.
- Warning-level rules and a passing check's findings come only from the
  capped findings list.

### Mitigations

- The CLI guide says plainly which sessions the verb can see and points to
  `koto session pin` and `--no-cleanup` for runs that must stay readable,
  and to the event sink's records for removed ones.
- New fields go in as additions; the response's keys are pinned by a
  literal test so a rename fails CI.
- A store-wide mode, and `--workspace` over a session listing once koto#343
  lands, can be added around the same per-session records without changing
  what existing callers get.
- An open batch on another host's session would make a flush refuse as a
  conflict anyway, so the unflushed read is no less current than the
  flushing one in that case.
- `findings_truncated` and `rule_counts_truncated` are always present, so a
  caller knows when a list is incomplete instead of guessing.
