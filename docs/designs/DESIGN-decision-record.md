---
schema: design/v1
status: Planned
problem: |
  koto has no record of who made a delegated decision or on what authority, and no way for a
  session above to reverse one so koto honours it. The record has to live as write-once keys in
  the deciding session, be read and reversed from above through session retention without any
  session writing into another, block a live workflow that built on a reversed decision, and
  replace a host file (the decider ledger) that exists only because sessions used to vanish.
decision: |
  Each decision is one create-only key in a koto-reserved area of the session that made it:
  `koto/decisions/<id>`, or `koto/aimed/<target-session-id>/<id>` when it is a reversal or a
  replacement aimed at another session's decision. Local writes use an atomic create-rename
  under the session's manifest lock, which also allocates a dense 12-digit identifier; the bucket
  holds each decision as its own object put with `If-None-Match: *`. A live decider's `koto next`
  lists one near-empty directory per chain session to find what is aimed at it, skips the read
  when it holds no decisions, and blocks through a reserved synthetic condition. The decider comes
  from koto's own variables (a new koto-skills hook supplies the acting agent), authority from the
  anchor, the chain from koto's dispatched-by links. Gate-passing commands write their event and
  their decision in one critical section, the decider ledger is retired, and everything is
  reached through an extended `koto decisions` command. A successor that took over through the
  supersede fence stands where its predecessor stood: the fence writes a write-once marker beside
  the predecessor, and the chain walk follows it forward with the session id checked.
rationale: |
  A per-target directory is the only layout whose per-tick cost doesn't grow with how many
  decisions the sessions above hold or how large their logs are, and every primitive it needs
  (atomic create-rename, the manifest flock, conditional puts, the pin's out-of-manifest object)
  already exists in koto. Logs, appended blobs and mutable indexes were each rejected because
  their read cost grows, their write-once guarantee rests on client code, or they can go stale.
  Keeping everything inside koto's own keys and variables honours the owner's ruling that
  decisions live in the session, the one-writer rule, and tool independence; leaving the run
  journal to the event-sink work keeps telemetry receiving exactly what it receives today.
upstream: docs/prds/PRD-decision-record.md
user_visible_surface: true
decision_provenance: inline-resolved
---

# DESIGN: The decision record

## Status

Planned

## Context and Problem Statement

The requirements are in [PRD-decision-record](../prds/PRD-decision-record.md). This section
restates the technical problem they create.

Every delegated decision (an acceptance, a merge, a gate pass, an escalation or a decision not to
escalate, a judgment call, a verdict) must be kept as keys in the koto session that made it, and
never in a file under `~/.koto/` outside the session directories (R4). Those keys are write-once:
no later write by koto, by `koto context add` or `remove`, or by a template may change them (R5).
Under the cloud backend they push like other keys and refuse a second write there too, and a
migrated source serves them read-only (R6).

A decision names its decider at two levels, the koto session plus the agent within it, or a
person by an opaque handle with no account, or an automated check (R7, R8), each value labelled
`observed` or `handed` (R9). It records the run mode (R10), the authority its decider held (R11),
and a snapshot of its chain: the sessions above it, read from koto's own dispatched-by links in
the deciding session and in its anchor, each link labelled by how koto wrote it (R12). koto reads
nothing of the workspace manager's to build any of this (R13).

A session above reads those keys after the deciding session finished, through any number of
finished sessions between them, for as long as retention keeps them (R14, R15), and the read
changes nothing in the read session or in any session between (R16). It reverses a decision from
its own session: the reversal is a decision of kind `reverse`, recorded in the reverser's session
if that session is on the original's chain and otherwise in the reverser's anchor (R4). Standing
follows the original's chain: the decider itself, a session above it, or a person entering through
one, and only a person can reverse a person (R17). Two reversals of one decision are settled when
read, by earliest time, then reverser session name, then identifier, with the loser reading
`refused` and its own command recording that refusal in its own session (R19).

While the deciding session's workflow is live, every `koto next` reads the reversals aimed at its
decisions from the sessions on its chain, refuses to advance when one is reversed, and stays
blocked until the workflow rewinds to the state the decision recorded or the reverser, or someone
above it, records a replacement naming the reversal (R22). That read writes nothing anywhere and
fails closed: an unreadable chain session blocks, a vanished one ends the chain (R23).

Three gate-passing commands (`koto overrides record`, `koto next --to` past an unmet gate, `koto
decider answer` with `pass`) record a `gate-pass` with a named, defaulted or refused decider, and
their existing records and the decision land together or not at all (R24). koto's existing
records map onto the vocabulary (R25), the decider ledger file is neither written nor read (R26),
shirabe's decision points write decisions without changing what their own readers see (R27), a
coordinator's interim entries import idempotently (R28), and a report counts reversals by kind over
the sessions koto keeps (R29). All of it works with koto alone (R31).

What exists today, at koto v0.32.0:

- **Links.** Each dispatched session holds koto-owned keys `koto/links/dispatched-by/<peer>`, and
  each session an agent session opens holds `koto/links/held-by/<anchor>`. koto writes them at a
  request-leg bind, at `koto init --dispatched-by`, through `koto session link`, and at anchor open.
  There is no reverse index: finding the sessions below one means scanning the store.
- **Reserved keys.** `koto/` is the one reserved prefix. The agent writer is refused there, and
  templates can't assign under it. There is no write-once or compare-and-swap for keys: a local
  write replaces the file and rewrites the manifest (`src/session/local.rs:700`), and a cloud key
  push is a plain put. Only state-file pushes use conditional writes (`put_state_if`).
- **Reads from above.** Since the retention work, a cross-session `koto context get` writes nothing
  into the read session; it logs to the reader's anchor or to a host spool.
- **Retention.** A finished session stays while any session in its connected hierarchy is alive,
  then for a configured keep period. The scan walks every file under each session's `ctx/koto/`
  (`src/session/hierarchy/facts.rs`).
- **Decision-like records.** `GateOverrideRecorded`, `DecisionRecorded`, `DirectedTransition`,
  `DeciderCheckAnswered` and `FindingDisputed` events carry no actor, or only a writer category.
  The decider ledger, `~/.koto/_decider_ledger.jsonl`, has five writers and one reader, `koto
  decider report`.
- **Run journal.** `~/.koto/_run_journal.jsonl` relays check, override and decider-answer events,
  answers included, for external telemetry. koto's event-sink work sends the same records to an
  external sink inline and removes the journal in a later release.
- **Blocking.** `koto next` builds blocking conditions only from gate results; the reserved
  `__action__` condition is the one synthetic precedent (`src/engine/advance.rs`).
- **Cloud and import.** Keys live under the session's bucket prefix. Import copies every key
  verbatim except a fixed list of host-local `koto/` keys and gives the copy a new session id, and
  since koto#540 a migrated source serves reads and refuses writes.

## Decision Drivers

- **The owner's ruling.** Decisions live in the koto session, not in a flat store under koto on the
  host. Session retention keeps them readable, and the cloud backend carries them when a session
  moves.
- **One writer per session.** A session's keys are written only by processes acting for that
  session. koto's cloud sync isn't multi-writer, a migrated source refuses writes, and another host
  can't reach a local-only session. Nothing may write a reversal, a read marker or an index into
  another session.
- **Write-once without a lock between sessions.** Immutability has to hold for writers in one
  session racing each other, locally and in the bucket, with no store-wide lock.
- **A cheap tick.** The reversal read runs on every `koto next` of a live workflow: under 50 ms at
  chain depth five locally (R36), and under the cloud backend inside koto's 30-second action limit,
  which the state-unification work only recently brought a writing tick down to about 15 seconds
  against.
- **Retention's scan.** Every key under `ctx/koto/` is walked on each retention scan, a symlink
  there taints a component, and nesting deeper than four levels does too. Decision keys must not
  slow or break it.
- **Import.** Keys must travel through `koto session import` and resolve by session name
  afterwards, though the copy gets a new session id.
- **Compatibility.** Every existing command keeps its arguments and output; the one break is
  refusing a gate pass with no establishable decider.
- **Opted-in backends.** The decider, telemetry and the model provider are third-party backends the
  owner opted into. Nothing here may gate, hold or change what they receive.
- **Tool independence.** Nothing may require niwa. A niwa-managed workspace may install koto's
  hooks and pass koto's variables through its configuration, and that is all.

## Considered Options

### Decision 1: Where decisions sit, what makes them write-once, and how a live decider finds reversals

Delegated to /decision.

A decision must be keys in its session (R4), immutable against every writer locally and in the
bucket (R5, R6), with a fixed-width identifier whose byte order is record order even under two
concurrent writers (R3, R32). A live decider must find reversals aimed at it on every tick, in
sessions on its chain only, under 50 ms locally at depth five with 1,000 decisions in each session,
writing nothing and failing closed (R22, R23, R36).

Key assumptions:
- A reversal recorded in the reverser's anchor (R4's fallback) sits in a session on the original's
  chain, as R4 states; otherwise the tick couldn't find it without scanning the store.
- One host writes a given session at a time under the cloud backend; same-host writers serialize
  on the manifest lock.
- The bucket providers koto supports (AWS S3, Cloudflare R2, MinIO) honour `If-None-Match: *` on a
  put. A provider that answers 501 can't hold decisions; there is no unconditional fallback.
- Local costs are estimates (a `read_dir` of a small or missing directory in tens of microseconds);
  R36's CI benchmark confirms them. Cloud round trips cost 50 to 200 ms, and chains stay shallow.

#### Chosen: One create-only key per decision, with aimed decisions partitioned by target session id

Two reserved areas in each session, plus one held empty:

- `koto/decisions/<id>`: every decision not aimed at another session's decision, `refused` records
  included.
- `koto/aimed/<target-session-id>/<id>`: every decision aimed at another session's decision: each
  `reverse`, and each replacement whose `basis` names a reversal (R22's clearing path). The target
  is the original deciding session, addressed by its header `session_id`.
- `koto/packs/`: reserved, guarded and empty, so a bulk listing format can be added later without
  moving a key.

The identifier is a dense, zero-padded 12-digit sequence, unique across both areas, allocated inside
the session's `ctx/manifest.lock` critical section by reading both areas and taking the highest id
plus one. Concurrent writers in one session serialize on that lock, so ids are distinct and in
commit order. Time is a body field, so clock skew can't reorder ids.

Locally a decision is written to a synced temporary file in the session directory, outside
`ctx/koto/`, then published with koto's existing `atomic_create_rename` (`src/engine/atomic_fs.rs`),
which refuses when the key exists; the directory is synced before success is reported. Decision
files stay out of the local and pushed manifests, as the pin does, so `read_dir` is the single
source of truth. A store-level guard, extending `check_writer_key` (`src/session/links.rs`),
refuses every writer and every remove under the three areas, through `add_as`, `remove` and the
CLI, naming the key; only the create-only decision writer passes. In the bucket each decision is
its own object put with `If-None-Match: *`: a 412 whose bytes equal the local bytes is success (a
replayed flush), a 412 with other bytes refuses, a 501 refuses, and the decision path never takes
the unconditional fallback the state-file put has today. Before its first decision put, koto probes
a bucket once (Security Considerations). Decision objects go up before the manifest and the state
file in a tick's flush, so a refusal stops the tick before its state moves, and a pull never
overwrites a decision key. Because decision files sit outside the manifest, every path that copies
or rebuilds a session from its manifest has to list the decision areas the way it lists the pin:
the cloud pull onto a host that has no local copy, import, and a `--workspace` read. Each of those
paths is named in Issue 1 or Issue 7 of the plan.

A reader treats a session as unreadable (R23) when an id appears in both areas, a file name isn't a
12-digit id, a body fails to parse, a body's own session name or id disagrees with where it was
read, or a body's target id disagrees with its `aimed/<t>` directory.

Retention's walk stops at the three area roots, refusing a symlink or special file there and never
descending, so decision files cost the scan nothing and carry no edges. Import copies the areas
verbatim, carries the bucket objects the way it carries pins, writes them create-only at the
destination, and accumulates every earlier source session id in `koto/imported-from` instead of
naming only the immediate source, so reversals aimed at a session's pre-import id still find it.
Import rebuilds that key itself from the source's header and the source's own validated history,
and discards any copy an archive carries. Today the key holds a single source; its readers (the
hierarchy, the continues-bucket walk and cloud import) and the decision reader accept both that
single form and the new list, so sessions imported before this change keep working.

The per-tick read runs in `handle_next` before gates are evaluated, through a new pure read path
that never calls `ContextStore::get` or `read_events` on another session and writes nothing to any
mirror, spool, manifest or bucket:

1. **Skip** when the workflow is terminal or the session's own two areas hold nothing. If its own
   areas can't be listed, it doesn't count as empty.
2. **Aim ids** are the session's header id plus every id in `koto/imported-from`.
3. **Walk the whole chain**, starting at the session itself, cycle-guarded, in R12's order, reading
   `dispatched-by` and `held-by` by `read_dir`. A peer whose source carries a migration marker
   resolves to the session it moved to. A session with no directory and no bucket object ends the
   chain `incomplete`; any other failure blocks naming that session. The walk stops at 64 sessions
   and blocks as unreadable beyond that, so a hostile link chain can't run a tick past its time
   limit.
4. **List `koto/aimed/<aim-id>/`** in each chain session and read only what is found. Because the
   walk starts at the session itself, this includes the session's own aimed area: that listing is
   the own-session read R22 adds for a person's replacement, and it costs nothing beyond the walk.
   It fails closed like the others. From it koto counts a replacement only when a person decider
   entered it.
5. **Decide** with R17's standing, R19's race, R18 and R22's clearing rules; a decision that reads
   `reversed` and isn't cleared becomes a reserved synthetic blocking condition on the
   `__action__` precedent, its name refused at template compile time.

A rewind clears a block when the deciding session's own log holds a `rewound` event appended after
the decision, whose target is the decision's recorded state or a state the workflow last entered
before it. Each decision records the log sequence number it was recorded at, which makes "after"
and "before" exact even when a workflow loops through the same state: a later visit that records a
new decision gets a new id, and only that decision's own reversal can block it.

The tick reads only sessions on the chain, so an `asserted` reversal, which exists only because the
chain is broken and its reverser sits beyond the break, never blocks a live worker; listings find it
by scanning the store for `koto/aimed/<aim-id>/` and show the decision reversed. A reversal is
recorded in the reverser's session when that session is on the original's chain, otherwise in
`KOTO_ANCHOR`'s anchor when that is on the chain, otherwise (the `asserted` case only) in the
reverser's session; with none of these, `reverse` fails with `reversal_refused` and reason
`not_on_chain`.

Locally that is about 20 `read_dir` calls on near-empty or missing directories at depth five, well
under a millisecond, and it doesn't grow with how many decisions any chain session holds. Under the
cloud backend there are no requests when the skip applies; otherwise the walk is seeded from the
chain snapshot in the session's newest decision, every chain session's link and aimed listings,
the session's own aimed listing among them, go out in parallel, fresh (never from the five-second listing cache, since a reversal added within it
must not be missed), and a changed link is followed sequentially. That is about two round trips in
the common case and depth plus one in the worst, around a second at depth five.

#### Alternatives Considered

- **Decisions as events in the session's own log.** Append-only by construction, with the event
  sequence number as identifier. Rejected: R23's freshness check means a changed coordinator log of
  several megabytes has to be fetched whole on every tick under the cloud backend, the local cost
  grows with log bytes that are mostly not decisions, and it doesn't meet the requirement that
  decisions be keys.
- **One append-only key per session.** Rejected: the bucket's `If-Match` refuses only a stale
  writer, not a wrong one, so the second-write refusal would rest on client code; every append
  rewrites the whole key; and its escape hatch, sealed segments, ends up as create-only objects
  anyway.
- **Flat decision keys plus a per-session reverse index key.** Rejected: the mutable index can go
  silently stale, needs an exception in the write-once guard, and is last-writer-wins in the bucket.
- **Flat decision keys with the target in the key name.** Rejected: a local `read_dir` can't filter
  by prefix, so the tick would enumerate every decision name in every chain session, which grows
  with the busiest coordinator above it. Its cloud cost is the same as the chosen layout's, so it is
  the fallback if per-target directories prove a problem; only the path scheme would change.

### Decision 2: How koto captures the decider, the agent, the run mode and the authority

Resolved inline.

A session decider names the koto session, the harness session and the agent within it (R7); values
are `observed` or `handed` (R9); the run mode is separate (R10); authority and chain come from
koto's own records, never the workspace manager's (R11 to R13).

Key assumptions:
- The harness's PreToolUse hook payload carries the acting agent's identifier for a call made inside
  a sub-agent and lets a hook rewrite the command. Where it doesn't, decisions record `unknown`,
  which R7 allows.
- No harness signal reports whether a session is attended today; until one does, run mode is handed
  or `unknown`.

#### Chosen: koto's own variables and keys, with a koto-skills hook for the agent and run mode

- **Session decider:** the koto session name always; `CLAUDE_CODE_SESSION_ID` when set and
  `KOTO_ANCHOR` when it names a readable anchor, both `observed`. Either of those, or `--by`,
  establishes the decider.
- **Agent:** `KOTO_AGENT_ID` (`observed`), set by a new koto-skills PreToolUse hook on Bash calls
  that run `koto`, from the hook payload's agent identifier. It sets `main` only when the payload
  positively identifies the main agent, sets nothing when the payload is ambiguous, and only sets a
  value matching koto's identifier pattern. `--agent` is `handed`; neither gives `unknown`.
- **Run mode:** `KOTO_RUN_MODE` from the same hook, only when the harness reports it; `--run-mode`
  handed; otherwise `unknown`.
- **Authority:** `level` and `grants` copied, `handed`, from the anchor's `koto/anchor/authority`
  key, which `koto session anchor open --record-authority` writes from `KOTO_AUTHORITY`; `dispatcher`
  from the nearest dispatched-by link; the chain snapshot walked at record time, read-only.
- **Person:** `--by person:<handle> --surface terminal|forge|relay [--link <url>]`, entered through
  the deciding session.

#### Alternatives Considered

- **Flags only.** Rejected: agents rarely know their own identifier and nothing checks it.
- **The workspace manager's session map** (`.niwa/sessions/<id>.json`) for the dispatcher. Rejected:
  a harness id deleted at reap, outside koto, and R13 forbids it.
- **The telemetry resource attributes.** Rejected: the harness strips `OTEL_*` from tool
  subprocesses.

### Decision 3: Gate passes that land whole, and the decider ledger

Resolved inline.

A gate-passing command must resolve its decider before touching anything, and its existing record
and its decision must both exist or neither, even across a crash (R24). The decider ledger is
neither written nor read (R26). Telemetry receives what it receives today.

Key assumptions:
- Existing event payload structs accept an unknown optional field without breaking their readers.
  The run journal builds its records from named event fields (`src/run_journal/mod.rs`, the
  `GateOverrideRecorded { state, gate, .. }` arm), so an added field never reaches it.
- Session logs keep the grader's consultation and check events, so a report can be rebuilt from
  them.

#### Chosen: One critical section per command, repaired by the session's own next command

Each gate-passing command resolves the decider first. With none established it writes only a
`refused` decision and exits non-zero. Otherwise, inside the session's `ctx/manifest.lock` critical
section that Decision 1's allocator already takes, it:

1. repairs: creates, create-only, any decision key an event names but the areas lack;
2. allocates the id;
3. appends its existing event (`GateOverrideRecorded`, `DirectedTransition`, `DeciderCheckAnswered`)
   with one new optional field, `decision`, holding the whole decision; this append is the commit
   point it always was;
4. creates the decision key.

If the process dies between 3 and 4, the next koto command on that session that loads its log
(`koto next`, any `koto decisions` command, or the terminal tick before it finishes) runs step 1.
Only the session's own process does this, so one writer holds. A `reverse` is never repaired from an
event; it is written only by `koto decisions reverse` after its standing check.

The decider ledger's five writers stop appending. `koto decider report` reads the same facts from
session logs (`decider_consulted`, `decider_check_answered`, `gate_override_recorded`,
`directed_transition`, check events) across the store or `--below <session>`, plus the decision
keys. koto never reads or deletes an existing ledger file; its docs say it is no longer used. Each
ledger kind maps to what already sits in the session log:

| Ledger kind | Written today at | Read from instead |
|---|---|---|
| `consulted` | `src/cli/decider_port.rs` | the `decider_consulted` event |
| `answered` | evidence submission in `handle_next` | the `evidence_submitted` event paired with its `decider_consulted`, as the run journal's `decider_answered` relay already pairs them |
| `directed_exit` | the `--to` path | the `directed_transition` event and its `gate-pass` decision |
| `checked` | `src/cli/check_evaluator.rs` | the `gate_evaluated` event |
| `check_overridden` | `src/cli/overrides.rs` | the `gate_override_recorded` event and its `gate-pass` decision |

The repair step validates an event's embedded body exactly as a fresh record is validated before
creating its key.

The run journal is not changed. No record kind or field it or the event sink carries changes, so the
opted-in telemetry backend receives exactly what it receives today. The event sink's emit path
queues a clone of the journal record, which the journal builds from named event fields, so the sink
inherits that projection; a test asserts each projected record is byte-identical with and without
the new field, and is rerun when the event-sink work's releases land. The requirement that no gate
answer sits in a host file is met by the event-sink work: its first release adds
`run_journal.enabled`, and its second (koto#545) deletes the journal. This feature's host check of
that clause runs on a koto without the journal; before then only an isolated test home switches the
journal off, and nothing tells a host to, because a host archive reads the journal until that host
cuts over.

#### Alternatives Considered

- **Key first, event second.** Rejected: a key with no event would claim a decision whose command
  never landed, and removing it would break write-once.
- **The decision only as an event.** Rejected: R4 asks for keys a reader from above lists by name.
- **Keeping the ledger beside the session copies.** Rejected: two records, and the planning forbids
  reading or writing it.
- **Replacing the answer in journal records with a pointer.** Rejected: it designs a record format
  for a file koto#545 deletes, and it would change what reaches telemetry during the overlap window,
  which the owner's rule on opted-in backends forbids.
- **Treating the run journal as telemetry outside the requirement.** Rejected: the planning names
  files under koto on the host with no exception and lists the decider ledger explicitly, so the
  journal reads as inside it; the event-sink work removes the file anyway.

### Decision 4: The command surface

Resolved inline.

R14, R15 and R17 to R29 need commands to record, list, show, reverse, report and import, and koto
already has `koto decisions record <name> --with-data` and `koto decisions list <name>`.

Key assumptions:
- `koto decisions list <name>` called with no new flag must keep returning today's per-state list.

#### Chosen: Extend `koto decisions`

`record <name> --with-data` keeps its form and now also writes a `judgment`. `list <name>` keeps its
output when given none of the new flags. New forms:

| Command | Purpose |
|---|---|
| `koto decisions record <session> --kind <k> --outcome <o> <subject flags> [decider flags]` | record any kind (R1 to R3) |
| `koto decisions list <session> [--received] [--below] [filters] --format v1` | list with status (R14, R15) |
| `koto decisions show <session> <id>` | one decision with every recorded field and its status, its winning reversal with standing and path, and its affected dependents, read from any session on the host (R21, R44) |
| `koto decisions chain <session>` | the session's chain as koto walks it now, with link kinds, supersede hops and their confirmation, and the session's own supersede marker (R45) |
| `koto decisions reverse <session> <id> [--in <session>] [--compensating <url>] [--replacement <outcome>] [decider flags]` | reverse under R17 to R19 and R39 to R43 |
| `koto decisions report --since <t> --until <t> [--below <session>]` | R29's tables |
| `koto decisions import <session> <file>` | R28's idempotent import |

Shared decider flags: `--by session|person:<handle>|check:<name>`, `--surface`, `--link`,
`--agent`, `--run-mode`, `--basis` (repeatable), `--evidence` (repeatable), `--reason`. Output is
JSON carrying `"v": 1`. New typed errors: `decider_missing`, `decision_exists`, `reversal_refused`,
`invalid_decision`. `reversal_refused` gains the reason `supersede_unconfirmed` (R40), and every
command refuses a fenced session with the existing `session_superseded` (R37). `list` gains a
`--fenced <session>` filter for reversals whose standing path names a hop from that session (R41).
Every subcommand takes its session as a positional argument and never reads it from the
environment (R46).

#### Alternatives Considered

- **A new noun** (`koto record`, `koto decide`). Rejected: two names for one concept beside an
  existing `koto decisions`.
- **Folding into `koto context`.** Rejected: decision keys are koto-owned and write-once; generic key
  verbs would blur that.
- **Folding into `koto request`.** Rejected: root sessions have no request.

### Decision 5: How shirabe and a coordinator's interim entries reach the record

Resolved inline.

shirabe's acceptance, merge, review panel, finding dismissal and coordinator decisions must write
decisions in the session that ran the step, without changing what their readers see (R27), and a
coordinator's interim entries must import once (R28).

Key assumptions:
- shirabe's scripts can name the session they run in; they already do for every `koto context` call.

#### Chosen: shirabe calls `koto decisions` where it already decides

- `shirabe transition <doc> Accepted` gains `--record-in <session>`; skills pass their own session,
  and the binary runs `koto decisions record --kind accept` with repository, path and commit.
  Without the flag it records nothing, so direct use keeps working.
- `skills/execute/scripts/merge-exec.sh`, the one `gh pr merge` site, records `merge` after a
  successful merge, in the run's session.
- `skills/work-on/scripts/panel-scope.sh` records a `verdict` per seat with `--agent <seat>` when it
  writes the panel verdict, and a `judgment` when a seat dismisses its own finding.
- `skills/coordinate/scripts/state-decision.sh` records `judgment` for `--settle`, `escalate` for
  `--escalate` and `decline-escalation` for `--hold`; the reversal-row path calls `koto decisions
  reverse`. Each row gains one field naming the decision's session and id.
- The work-on template's directions to run `koto overrides record`, `koto decider answer` and `koto
  decisions record` need no change, because those commands now record decisions themselves.
- A shirabe script converts the entries a coordinator recorded under the interim rule into the JSON
  Lines file `koto decisions import` reads, run once when this record lands.

All of these are shirabe work items. None lands in niwa. shirabe may require koto.

#### Alternatives Considered

- **koto observing shirabe's steps through hooks.** Rejected: koto can't tell an acceptance from any
  other file edit.
- **Rendering the coordinator's tables from the record now.** Rejected for this feature: a rewrite of
  the coordinator's state scripts, which the state-unification work just moved, for no reader-visible
  gain.

### Decision 6: How a successor gets its predecessor's standing

Resolved inline. Added with the PRD's R37 to R48, and answers open question 3.

A coordinator replaced through `koto init <successor> --continues <predecessor> --supersede` must
stand where its predecessor stood and no higher (R39), read from the fence and never from the
`continues` link, which the successor writes itself. The tick's reversal read has to find a
reversal recorded in a successor (R39), the fenced session must record nothing (R37), and the walk
has to stay inside R36's budget (R48).

Key assumptions:
- Whoever can fence a session can already cancel or change it, locally or through the bucket
  credential, so reading standing from the fence grants nothing they lacked. If koto ever grants
  bucket writes per prefix, this needs revisiting.
- The fence keeps its one-winner rule: of two successors, the first append wins.

#### Chosen: A write-once supersede marker beside the fenced session, followed forward in the chain walk

- **The marker.** `koto init --supersede` writes `koto/superseded-by` in the predecessor beside the
  fence, create-only by the decision writer's primitive (local create-rename, `If-None-Match: *`
  in the bucket), holding the successor's name and session id, the fence's actor and time, and
  `decisions_through`, the highest decision id the fence saw in the predecessor. It sits
  outside the manifest like the pin and the migration marker, so one `stat` or one GET reads it.
  It's written after the fence and before the successor is created. A re-run of the command, which
  the fence already makes idempotent, writes a missing marker. `check_writer_key` refuses every
  other writer and every remove of it.
- **The fenced session records nothing.** Locally, the decision writer reads the fence under the
  `ctx/manifest.lock` it allocates under. Today `append_supersede_fence` takes only the log's
  append lock, and no command holds the manifest lock and the append lock together, so the order
  below is set by this design for the two new holders rather than copied from current code: the
  fencing command takes the manifest lock first and the append lock inside it, holding both
  through the append, and a gate-passing decision write that appends to the log takes them in the
  same order. Neither takes the other's locks in reverse, so the two can't deadlock, and a
  decision and a fence can't interleave. The state-file lock `koto next` takes for a whole tick on
  a batching state sits outside both and isn't part of this order: neither the fence nor the
  decision writer takes it, so a decision written during a tick that holds it waits only on the
  two locks above, and the fence never waits on a tick. The fence reads `decisions_through` under the manifest lock. Its wait on
  that lock is bounded by `FENCE_LOCK_WAIT` like its wait on the append lock, which needs a
  bounded acquire: `LocalBackend::acquire_flock` blocks without limit, so the fence takes the
  manifest lock through a timed acquire like `AppendLock::acquire_within`. A lock still held when
  the wait ends exits 3 `fence_unavailable` with nothing written. Under the cloud backend no lock spans the
  fencing host and the predecessor's host, and a plain `record` pushes no event whose conditional
  push the fence would make fail. So the fence lists the predecessor's decision prefix in the bucket
  before its conditional push and takes `decisions_through` from that listing, and every reader
  treats a decision in a fenced session with a higher id as written after the fence: status
  `refused`, no standing, no block (R37). A decision created between the listing and the push
  reads `refused` too, which errs toward the fence.
- **The walk.** `src/decisions/chain.rs` gains a forward step. At each chain session it reads the
  marker; if one is there, it reads the successor's header and confirms the session id, then takes
  the successor's position as the fenced session's and keeps going forward, up to
  `MAX_SUPERSEDE_HOPS` (64), the constant `src/session/links.rs` already defines. It doesn't reuse
  `newest_member`: that walk reads each fence from the local log and ends at the last member it
  reaches without saying whether a hop held, while this step reads the marker in one read under
  either backend and has to report each hop as confirmed or not. It applies the same name check
  and cycle guard. Upward it continues from the fenced session's frozen links, never the
  successor's. A missing marker ends the forward step there; a malformed one makes the session
  unreadable (R23); a mismatched id marks the hop unconfirmed.
- **Standing.** `src/decisions/standing.rs` treats each confirmed later member as the fenced
  session: above what it was above, the same decider for R17's first rule, and an entering session
  for a person exactly as the fenced session was. The rule that only a person reverses a person is
  unchanged (R43). A reversal that needs an unconfirmed hop is refused with `supersede_unconfirmed`
  and never asserted (R40). The reversal's `path` records each hop: fenced session, successor,
  actor, time (R41).
- **The tick.** For each chain session, the tick also lists `koto/aimed/<aim-id>/` in that
  session's confirmed later members, so a successor's reversal blocks the worker below (R39). A
  reversal of a fenced session's decision blocks no member of its run, since the fenced session
  has no live workflow left to stop (R42).
- **The chain read.** `koto decisions chain <session>` prints this walk without recording
  anything (R45). It's the same code path standing uses, so a consumer reads where a session
  stands instead of keeping its own walk.

#### Alternatives Considered

- **No standing for a successor; the person above reverses.** Rejected: after a coordinator and the
  one above it are both replaced, only the person at the top can overrule a long-lived worker, the
  run's routine reversals of its own earlier calls are refused, and a ruling recorded at the root
  can never be reversed after the first handover.
- **Re-link live workers to the successor at takeover.** Rejected: the worker re-links only after
  reading the fence, so the fence still grants the standing; a stopped worker never re-links;
  earlier decisions keep their old snapshot; and the predecessor's own decisions aren't covered.
- **Read the fence from the predecessor's log.** Rejected: a log scan per chain session on every
  tick breaks R36's budget and R38's one-read rule.
- **Standing `asserted` through a hop.** Rejected: an asserted reversal never stops a live worker or
  clears a verified one, so after a replacement only the person at the top could stop a worker.

## Decision Outcome

The six choices form one path. A decision starts at a recording command: a `koto decisions`
subcommand, one of the three gate-passing commands, or a shirabe step calling one of them. koto
resolves the decider from `--by`, or from the harness session and anchor it observes, with the agent
and run mode from the koto-skills hook or from flags, and refuses when nothing establishes one. It
copies the authority from the anchor, snapshots the chain from its own dispatched-by links, and,
inside the session's manifest lock, allocates a 12-digit id and creates one key: under
`koto/decisions/`, or under `koto/aimed/<target-id>/` for a reversal or a replacement. A
gate-passing command appends its existing event, carrying the decision, before the key, and the
session's own next command repairs a missing key. The bucket holds each decision as its own
create-only object.

A session above lists decisions by walking down its hierarchy through retention's inventory and
reading the two areas of each session, with a pure read path that writes nothing anywhere. It
reverses by recording a `reverse` in its own session, or its anchor, aimed at the original's session
id, after checking standing against the original's chain snapshot; if a competing reversal it can
reach won R19's race, it records its own refusal and exits non-zero. A live decider's `koto next`
lists `koto/aimed/<its-id>/` in each chain session, blocks on a reversed decision, and clears when it
rewinds or a replacement from someone with standing arrives in the same listing.

When a coordinator is replaced, the fence leaves a marker beside the predecessor. Every walk
(standing, the tick, listings below, `show` and `chain`) follows it forward to the successor with
the session id checked, so the successor reverses with the predecessor's standing and a reversal
it records reaches the worker below.

The decider ledger stops being written and its report reads sessions. The run journal and the event
sink are untouched, so telemetry receives what it receives today, and the host-file requirement is
met when the event-sink work removes the journal.

### What carries over from the first build

The first build (koto#394 and koto#396, closed unmerged) carries over in everything that didn't depend
on its host store:

| From the first build | Here |
|---|---|
| The decision kinds with closed outcomes | Carried, with `verdict` gaining `inconclusive` |
| The decider vocabulary, `observed` and `handed` labels, `defaulted` | Carried |
| The standing rules, verified through a complete chain and asserted otherwise | Carried, with the chain read from koto's links instead of the workspace manager's map |
| Reversal as its own final decision | Carried |
| The blocked step after a reversed gate pass | Carried and widened to any reversed decision of a live workflow, a product choice the PRD records |
| Reads that fail closed | Carried |
| The agent read from a koto-skills hook | Carried |
| Monthly JSONL files and an all-time reversal log | Dropped: decisions live in sessions |
| A store-wide lock as the race arbiter | Dropped: races are settled when read |
| The dual write to session and store with a repair tick | Replaced by a write and repair inside one session |
| Reading the workspace manager's session map for the chain | Dropped: koto's own links |
| The 100,000-decisions-in-five-seconds target over one host store | Restated over retained sessions (R36) |

The first build put decisions in a host store because the deciding session "may be gone" and koto had
"no cross-session write path". Retention answers the first. The second was wrong for keys, but the
one-writer rule rules the path out anyway, which is why reversals live in the reverser's session.

### The questions the planning left open

- **What becomes of the decider ledger's readers.** Its one reader, `koto decider report`, reads
  sessions (Decision 3). The measurement pipeline and its skill read neither the ledger nor the report
  (confirmed by the measurement lane, 2026-10-30).
- **The layout of the decision keys.** `koto/decisions/<id>` and `koto/aimed/<target-id>/<id>`, with
  `koto/packs/` reserved (Decision 1).
- **What makes them write-once.** A create-only rename locally and `If-None-Match: *` in the bucket,
  behind a guard that refuses every other writer (Decision 1).
- **How far up a live decider reads, and what it costs.** The whole chain, with no depth bound; under
  a millisecond locally, about two round trips under the cloud backend, and nothing at all for a
  session that holds no decisions (Decision 1).
- **Where overrule counts are read from.** A scan of the sessions koto keeps (R29). An index kept in a
  session and rebuilt from them is deferred to the reporting work that needs a longer memory, and
  `koto/packs/` leaves room for a bulk format.
- **How it composes with what the harness records.** Decisions carry pointers to the harness session,
  tool call and agent (R30) and never copy what they point at; the agent comes from the harness's own
  hook payload (Decision 2).
- **What reversing each kind means.** PRD R21 and R22.
- **How the dispatch tree's attribution reaches the record, given the workspace manager never reads or
  writes workflow state.** Through koto's dispatched-by links, which koto writes at a request-leg
  bind, at `koto init --dispatched-by`, through `koto session link` and at anchor open, from
  `KOTO_DISPATCHER_ANCHOR`. A workspace manager may pass that variable through its configuration; its
  code names no koto variable and koto reads none of its files.

## Solution Architecture

### Overview

A new koto module, `src/decisions/`, owns the decision body, the identifier allocator, the
create-only writer, the chain walk, standing and race resolution, and the pure read path. The
session stores gain a create-only write for the three reserved areas and a guard against every other
writer. `koto decisions` is the front end. Three gate-passing commands and `koto decisions record
--with-data` call the writer, `koto next` calls the reversal read, and `koto decider report` reads
sessions. The koto-skills plugin gains one hook. shirabe calls the commands at its decision points.

### Components

- **`src/decisions/body.rs`** (new): the decision body (R1 to R3, R7 to R12, R30), its validation
  against R1 and R2, and a size cap: a body over 16 KiB, or a reason over 4 KiB, is refused, which
  keeps per-host session archives bounded.
- **`src/decisions/store.rs`** (new): the allocator and create-only writer over both backends;
  integrity checks for readers (R23); listing of a session's two areas.
- **`src/decisions/chain.rs`** (new): the chain walk over `koto/links/dispatched-by/` and
  `koto/links/held-by/`, in R12's order, labelling each hop, marking `incomplete`, cycle-guarded,
  and following each chain session forward through its supersede marker with the session id
  checked (Decision 6). It backs `koto decisions chain`.
- **`src/cli/init_supersede.rs`, `src/session/links.rs`**: the fencing command writes the
  create-only `koto/superseded-by` marker and takes the manifest lock around its fence append;
  `check_writer_key` refuses every other writer and remove of the marker (R37, R38).
- **`src/decisions/standing.rs`** (new): R17's standing over a chain snapshot, R18's refusals, R19's
  race, R22's clearing rules, and each decision's status.
- **`src/decisions/decider.rs`** (new): resolves the decider, agent, run mode and authority (Decision
  2), and refuses with `decider_missing`.
- **`src/decisions/read.rs`** (new): the pure read path, local `read_dir` and file reads or cloud
  delimited listings and gets, writing nothing anywhere.
- **`src/session/local.rs`, `src/session/cloud.rs`, `src/session/sync.rs`**: create-only decision
  writes; flush ordering; a pull that never overwrites a decision key; a `/` delimiter on the probe
  listing.
- **`src/session/links.rs`**: `check_writer_key` extended to refuse every writer and every remove
  under the three areas.
- **`src/session/hierarchy/facts.rs`**: the walk stops at the three area roots.
- **`src/session/import_links.rs`** and the cloud import: carry decision objects; accumulate
  `koto/imported-from`.
- **`src/cli/mod.rs` (`handle_next`), `src/engine/advance.rs`**: the reversal read before gate
  evaluation, the reserved synthetic blocking condition `__decision_reversed__` (and
  `__decision_unreadable__` for R23), both names refused by `src/template/types.rs`.
- **`src/cli/overrides.rs`, the `--to` path in `src/cli/mod.rs`, `src/cli/decider_answer.rs`,
  `src/cli/disputes.rs`, `koto decisions record --with-data`**: write decisions per R24 and R25.
- **`src/decider/ledger.rs`, `src/cli/decider.rs`, `src/decider/report.rs`**: writers removed, report
  rebuilt from sessions.
- **`plugins/koto-skills/hooks/`**: a PreToolUse hook setting `KOTO_AGENT_ID` and `KOTO_RUN_MODE` on
  Bash calls that run `koto`, registered in `hooks.json`; it exits 0 silently when it can't decide.
- **Docs**: `docs/reference/` gains the decision body and error codes; `docs/workspace-layout.md`,
  `docs/guides/cli-usage.md`, `docs/guides/decider-authoring.md` and the koto-skills references drop
  the ledger and add the commands.

### Key Interfaces

A decision body (field names indicative):

```json
{
  "v": 1,
  "id": "000000000042",
  "session": {"name": "work-on-123", "id": "6f1c..."},
  "time": "2026-11-20T17:04:11.512Z",
  "state": "review",
  "log_seq": 411,
  "kind": "gate-pass",
  "outcome": "passed",
  "subject": {"workflow": "work-on-123", "state": "review", "gate": "panel"},
  "decider": {
    "kind": "session",
    "session": {"value": "work-on-123", "source": "observed"},
    "harness": {"value": "9c1e...", "source": "observed"},
    "anchor": {"value": "anchor-9c1e...", "source": "observed"},
    "agent": {"value": "unknown", "source": "observed"},
    "defaulted": true
  },
  "authority": {"kind": "dispatch", "level": null, "grants": [],
                "dispatcher": "coordinate-roadmap-x", "run_mode": {"value": "unattended", "source": "handed"}},
  "chain": {"links": [{"session": "coordinate-roadmap-x", "via": "leg"},
                      {"session": "anchor-9c1e...", "via": "held-by"}],
            "incomplete": false},
  "basis": [],
  "evidence": [{"kind": "event", "session": "work-on-123", "seq": 412}],
  "reason": "panel finding is a false positive"
}
```

A `reverse` adds `target` (`{name, id, decision}`), `standing` (`verified` or `asserted`), the
`path` that gave it standing, and optional `compensating` and `replacement`. A path step reached
through a supersede hop records the fenced session, its successor, and the fence's actor and time:

```json
"path": [{"session": "work-on-123", "via": "leg", "to": "coord-a"},
         {"hop": {"fenced": "coord-a", "successor": {"name": "coord-b", "id": "a41f..."},
                  "actor": {"...": "as the fence records it"}, "time": "2026-11-20T12:00:03.114Z"}}]
```

The supersede marker, `koto/superseded-by` in the fenced session, holds
`{"v": 1, "successor": {"name", "id"}, "actor", "time", "decisions_through"}`, the last the
12-digit id or `null` when the session held none. `koto decisions chain <session>` prints
`{"v": 1, "session", "marker", "links": [{"session", "via", "hops": [{"successor", "confirmed",
"actor", "time"}]}], "incomplete"}`. A `person` decider
carries `handle` (`handed`), `surface`, an optional `link` and the entering session. A `check` decider
carries `name` and the session that ran it. An imported decision carries `source`, `source_id` and
`imported_at`.

Session events `GateOverrideRecorded`, `DirectedTransition` and `DeciderCheckAnswered` gain one
optional field, `decision`, holding the body. Existing payload keys keep their names and paths.

The commands are Decision 4's table. Typed errors: `decider_missing` (exit 2), `decision_exists`,
`invalid_decision` (naming the field), `reversal_refused` (with the reason and, for a lost race, the
winner), plus the existing `session_migrated`.

### Data Flow

1. **Record.** A command resolves the decider, takes the manifest lock, repairs, allocates, appends
   its event when it has one, creates the key, pushes the object under the cloud backend, and returns
   the id.
2. **Read from above.** `list --below` builds the hierarchy below the reader from retention's inventory
   (`Graph::scan`), reads each session's two areas through the pure read path, computes each
   decision's status from the reversals found in the sessions on its chain, and writes nothing.
3. **Reverse.** `reverse` reads the original and its chain snapshot, checks standing, records the
   reversal in the reverser's session if it is on the chain and otherwise in `KOTO_ANCHOR`'s anchor,
   then lists competing reversals on the original's chain; if one wins, it records `refused` in its
   own session and exits non-zero.
4. **Tick.** A live session holding decisions lists `koto/aimed/<aim-id>/` in each chain session,
   in each confirmed later member of a chain session's run, and, once one of its decisions reads
   reversed, in its own session for a person's replacement (R22), before gate evaluation, and
   blocks or proceeds; its workflow state is untouched by a block.
5. **Show and chain.** `show` reads one decision from any session on the host, computes its status
   from the reversals on its chain and the chain's runs, and writes nothing; `chain` prints the
   walk itself.
6. **Report.** `report` and `decider report` scan the store, or the hierarchy below a session, read
   decision keys and session events, and print.

## Implementation Approach

### Phase 1: The write-once store

The reserved areas, the create-only writer in both backends (running the migrated-source refusal
and per-area entry limits matching the reader's), the guard, the bucket probe, retention's stop and
import's carry. Everything else stands on it. The retention stop and the import carry change code
the session retention work owns, so they start only once that work reads Done; the rest of the phase
doesn't wait.

Deliverables: `src/decisions/store.rs`, changes to `local.rs`, `cloud.rs`, `sync.rs`, `links.rs`,
`facts.rs`, `import_links.rs`; tests for R5, R6 and R32.

### Phase 2: Recording and listing

The body, decider resolution, chain snapshot, `koto decisions record`, `list`, `show`, with filters
and `--received`. `show` reads from any session on the host and returns every recorded field (R44);
the harness identifier follows the run journal's id-shape rule for a driver (R47).

Deliverables: `body.rs`, `decider.rs`, `chain.rs`, `read.rs`, the CLI; tests for R1 to R3, R7 to R16,
R44, R46 and R47.

### Phase 3: The agent hook

The koto-skills PreToolUse hook and its tests. Independent of Phases 1 and 2 beyond the variable
names.

### Phase 4: Reversal

`koto decisions reverse`, standing, refusals, the race and the loser's own refusal record, the
repeat check on each later decision command in the losing session, status computation in listings,
and the store scan that finds `asserted` reversals for listings.

Deliverables: `standing.rs`, the CLI; tests for R17 to R21.

### Phase 5: The live decider

The per-tick reversal read, the synthetic blocking conditions, clearing by rewind or replacement,
fail-closed reads, and the cloud listing path.

Deliverables: `handle_next` and `advance.rs` changes, `template/types.rs`; tests for R22 and R23, and
R36's tick benchmark.

### Phase 5a: Succession

The supersede marker and the fenced session's refusal, the forward step in the chain walk, standing
through confirmed hops, the hop in the standing path, the tick's read of later members, and
`koto decisions chain` (Decision 6). It needs Phases 4 and 5 for standing and the tick read.

Deliverables: changes to `init_supersede.rs`, `links.rs`, `chain.rs`, `standing.rs`, the tick read
and the CLI; tests for R37 to R43, R45 and R48.

### Phase 6: Existing records and the ledger

Gate-passing commands write decisions in one critical section with repair; `decisions record
--with-data` and `disputes record` write decisions; the ledger's writers go and `decider report`
reads sessions.

Deliverables: changes to `overrides.rs`, `decider_answer.rs`, `disputes.rs`, `mod.rs`, `ledger.rs`,
`report.rs`; tests for R24 to R26 and R33.

### Phase 7: Reporting and import

`koto decisions report` and `import`, and the listing benchmark over 1,000 sessions. R36's listing
clause depends on that benchmark: one file per decision is estimated at one to one and a half
seconds locally, inside the limit. If it nears five seconds, a create-only pack format under
`koto/packs/` is designed as a follow-up before R36 is claimed; nothing here defines one yet.

### Phase 8: Docs

The reference, guides and koto-skills references.

### Phase 9: shirabe

`shirabe transition --record-in`, the merge step, the review panel and dismissals, the coordinator's
decision scripts, and the interim-entry converter (Decision 5). Lands after Phases 2, 4, 6 and 7
have shipped in a koto release; the converter needs Phase 7's import.

### Phase 10: Acceptance

The PRD's acceptance criteria walked end to end once session retention reads Done; the first criterion's
host-file check is final only on a koto release without the run journal (koto#545), and an isolated
test of it before then is interim.

## Security Considerations

**Attribution, not authentication.** Every decider value is `observed` (koto read it from its own
environment, keys or hook) or `handed` (a caller supplied it), and neither proves identity. A
process that can run a shell command can set `KOTO_AGENT_ID`, `KOTO_ANCHOR` or `KOTO_AUTHORITY`,
open an anchor with any recorded level, or pass `--by person:<handle>`. The links that give standing
are keys written by the processes acting for each session. Standing therefore says who a record
claims is above whom among one user's sessions. A reversal's standing is `verified` only when its
path is complete and the live link keys agree with the chain snapshot stored in the original, and
`asserted` otherwise; the docs say so beside every command that prints standing. Nothing here
defends against a process with write access to `~/.koto` or to the bucket credential, which can add
or delete files directly. Write-once protects against koto's own commands, `koto context`, racing
writers in one session, and a second put of an existing bucket object. A per-session hash chain over
decision bodies is available as later hardening if tamper evidence is wanted.

**Untrusted input.** A decision body read from another session, an imported archive, a bucket object
or an import file is data. The reader enforces the body cap (16 KiB) and reason cap (4 KiB) and a cap
on entries per area; it `lstat`s the area root, each target directory and each file, opens files
without following symlinks and confirms a regular file on the open descriptor, and validates every
session id and decision id against a strict grammar before joining it to a path. A body whose
session name or id disagrees with where it was read, an id in both areas, or an over-size or
unparseable file makes that session unreadable, which blocks the live decider (R23). A hostile
session can use that to stall a worker; the design accepts it, because the alternative is to ignore a
reversal, and the block names the offending session so a person can resolve it. A reversal with
standing `asserted` sits beyond a broken chain, so it never reaches the tick; listings show it, with
its standing, and a person decides what to do. Writers enforce the same per-area entry limits the
reader does, so a runaway session can't make itself unreadable to the sessions below it. A
decision's time is stamped by koto's writer from the clock and never taken from a flag, so winning
R19's race by backdating needs a direct file write, which is outside what this protects against.
Human-readable `list` and `show` output strips control and terminal escape characters from reasons
and handles; JSON output escapes them. `koto decisions import` runs the same validation as a local record, labels everything
`handed`, never claims `verified`, lets an imported `reverse` name only an entry from the same
import and never another session's decision, and caps line length and count. `koto/imported-from` is
written only by the import command, holds validated session ids, and is capped, because it decides
which reversals a session treats as aimed at itself.

**No self-clearing.** A blocked worker can't clear its own block: a replacement clears only when its
decider has standing over the reverser, and naming the reversal in `basis` isn't enough on its own.
The one exception is a person who reversed their own decision through the deciding session: a
person entering through that session may record the replacement (R22), and the session alone still
can't. The tick finds that replacement in the session's own aimed area and counts it only when its
decider is a person.

**Standing through a fence.** Whoever fences a session takes its `verified` standing over everything
below it, a live worker's pauses included. That's no wider than what the fence already gives them,
since they can cancel the fenced session itself, and the standing path names the fence's actor and
time so a reader can list every reversal one fence enabled (R41). The marker is create-only and
guarded like a decision key; the walk checks the successor's own session id against it, so a
renamed or planted successor confirms nothing. A hostile marker can only make its session
unreadable, which blocks rather than grants.
Reversing a person's decision takes a person decider; because a handle is unauthenticated, the record
keeps the entering session and the observed harness id, so a reader sees which agent session claimed
to be a person.

**Platform guarantees.** The bucket's write-once guarantee rests on the provider honouring
`If-None-Match: *`. koto probes a bucket once before its first decision put, putting a probe object
twice and requiring the second to fail with 412, and refuses to hold decisions on a bucket that
doesn't. Where `atomic_create_rename` would fall back to a replacing rename after a cross-device
error, the decision writer treats the error as a failure. Another credential holder can still create
objects under a session's prefix in a shared bucket; the integrity checks make such an object surface
as an unreadable session rather than as a decision.

**The koto-skills hook.** It rewrites a user's Bash command, so it is the highest-impact new code. It
parses the payload as JSON, matches only commands that invoke the `koto` binary, adds a prefix of
environment assignments built only from values that match koto's identifier pattern, leaves the
command text byte-identical after that prefix, and changes nothing when anything is ambiguous. Its
tests feed it hostile command strings: compound commands, pipes, `env`-prefixed and quoted
invocations, subshells, and values carrying shell metacharacters. shirabe's scripts pass repository, path, commit and finding
text to `koto decisions` as separate arguments, never through `eval` or string building, and the
interim-entry converter writes JSON with a JSON encoder.

**Data exposure.** Decision bodies hold free text and pointers: reasons, evidence, links, handles and
the names of every session on the chain. They aren't scrubbed, and they travel with the session
through archives, import and the bucket; the docs say so. The opted-in telemetry, decider and model
backends receive what they receive today: the three events that gain a `decision` field are projected
into the run journal and the event sink by named fields, so the field never reaches them, and a test
asserts each projected record is byte-identical with and without it. New output that reaches the
model (the two synthetic conditions, `list` and `show`) is fixed wording plus ids; a reversal reason
shown there is capped and labelled as untrusted text. An old decider ledger file may still hold gate
answers; koto no longer reads or writes it, and the release notes tell the owner it can be deleted.
Project configuration is never read for standing, deciders or authority, so a cloned repository can't
widen who may reverse.

## Consequences

### Positive

- Every delegated decision has a named decider and authority and survives its session for as long as
  the hierarchy above needs it, with no host store.
- A reversal from above stops a live worker at its next step, cheaply: nothing for a session with no
  decisions, under a millisecond locally otherwise.
- The bucket itself refuses a second write of a decision.
- One vocabulary replaces five partial records, and the decider ledger goes.
- Fixes that help beyond this feature: the probe listing gets a delimiter, and a decision write never
  falls back to an unconditional put.

### Negative

- Listing many decisions reads one file or object per decision; a bulk path may be needed under the
  cloud backend.
- Decision files are invisible to `koto context list`, which reads the manifest.
- A planted symlink inside a decision area no longer taints retention, so the decision reader refuses
  symlinks itself.
- Two hosts writing one cloud session see refusals rather than silent overwrites.
- A reversal of a minor call stops the worker too.
- The host-file requirement can't be verified on a host until the run journal is removed.
- An `asserted` reversal, recorded beyond a broken chain, shows in listings but doesn't stop a live
  worker.
- A fence written by a koto release older than the marker confers no standing, so reversals below
  it still go to the session or person above.
- Accepted limits, each with the trigger for revisiting it: one writable chain session can block a
  worker that can't clear the block itself (revisit if sessions are ever shared across users through
  a bucket); a `--by person:<handle>` claim is unauthenticated, so an agent can claim to be a person
  and the audit trail is the only control (revisit when a harness or forge supplies verified
  identity); and the host-file check waits on koto#545 (revisit if that release slips past the
  measurement pipeline's cutover).

### Mitigations

- R36's listing benchmark runs in Phase 7, and `koto/packs/` is reserved for a bulk format.
- `koto decisions list` reads the areas directly; docs say `koto context list` doesn't show them.
- The integrity checks treat a symlink or special file in an area as unreadable.
- The refusal names the other writer's object; one-host-per-session is already the cloud rule.
- R22's rewind and the reverser's replacement are documented as the ways out of a block.
- The plan names koto#545 as an explicit dependency of the final verdict on that requirement.

## Where this touches neighbouring work

**The state-unification work** (making the koto session the record, with the bucket as the carrier
between hosts):
- *Cloud storage.* Decision objects add conditional puts to a tick's flush and listings to a decider's
  tick. The flush-ordering change (decisions before manifest and state) and the probe listing's new
  delimiter touch the cloud tick that work just made faster (koto#549). The reversal read costs
  nothing for a session with no decisions and about two round trips otherwise.
- *Successor coordinators.* A coordinator's successor continues its run through a `continues` link
  that, by that work's rule, grants no authority. The successor's standing comes from the
  supersede fence instead (Decision 6), which answers open question 3: a successor stands where its
  predecessor stood, read from a marker the fence writes, and never from the link.
- *The coordinator's record.* Its `record/decisions` and `record/reversals` keys stay; each row gains a
  decision citation (Decision 5).
- *Import.* Accumulating source ids in `koto/imported-from` changes a key the import code writes.
- *Reads write nothing.* The decision read path follows that work's rule that a read from above writes
  nothing into the read session, and goes further: it writes nothing to the reader's anchor or the
  host spool either.

**The session retention work this feature depends on.** Retention's walk gains a stop at the three
decision areas, and import carries decision objects like pins. Neither changes a retention rule, and
both land as part of this feature's execution, after that work is settled.

**The agent-surface legibility work** (rendering koto sessions in Claude Code's `/workflows`
screen; its blocked-status fix is koto#553, scoped in koto#557): a reversal block is a blocking condition, so it renders through that
work's blocked-step shape (an `error` step with `blocked: true` and a sanitized one-line reason). The
synthetic condition's name and reason must go through its sanitizer and caps. Nothing in the
`koto decisions` command set changes that screen.

**koto's product plan:**
- *Reading sessions from any machine through one's own bucket.* Decisions travel as bucket objects
  under each session's prefix, so a multi-machine reader sees them with the session. That plan hasn't
  settled how a session names the machine that produced it; a decision would carry that stamp once it
  exists.
- *Per-user and per-project configuration that a cloned repository can't change silently.* Nothing in
  this design reads project configuration for standing, deciders or authority, so a cloned
  repository can't widen who may reverse. `koto decisions` output carries its own format version
  until koto has a whole-CLI contract version.

**The event-sink work and the measurement pipeline.** No run-journal or sink record changes; the
host-file requirement waits on koto#545, which can't land before the measurement pipeline's cutover.
The measurement pipeline reads `gate_override_recorded`'s session, state, gate and sequence number by
name, so an added `decision` field changes nothing for it, and it reads neither the ledger nor the
report.

## Open questions for the state-unification lane

Recorded for the state-unification lane, which was paused when this design was written. Questions
1, 2, 4, 5 and 6 carry the answer this design assumes, each pending that lane's confirmation.
Question 3 has since been answered by the PRD's R37 to R43 and Decision 6.

1. **Reserved areas outside the manifest.** Do `koto/decisions/`, `koto/aimed/` and `koto/packs/`, held
   as create-only bucket objects outside the pushed manifest the way the pin is, collide with anything
   planned for context keys? *Assumed, pending state-unification confirmation:* no collision; proceed.
2. **The coordinator's decisions and reversals tables.** Should `record/decisions` and
   `record/reversals` become a rendering over the record, or stay as rows that cite decision ids?
   *Assumed, pending state-unification confirmation:* stay and cite now; a rendering is later work.
3. **Successor standing.** Should a session that continues another, through a `continues` link, stand
   above what its predecessor dispatched when reversing a decision? That lane's rule says the link
   grants no authority. *Answer:* yes, read from the supersede fence rather than the link, with six
   conditions: a fenced session records nothing (R37); the fence is readable at listing cost
   through a write-once marker (R38); the walk follows the marker forward with the session id
   checked, every member standing in the fenced session's position and upward reads continuing from
   its frozen links (R39); a hop koto can't confirm refuses the reversal (R40); the hop is recorded
   in the reversal's standing path and a confirmed one gives `verified` (R41); and only a person
   reverses a person, unchanged (R43). Decision 6 has the mechanism.
4. **Tick budget.** Is about two parallel listing round trips per tick, only for a live session that
   holds decisions, acceptable within the cloud action limit? *Assumed, pending state-unification confirmation:* yes.
5. **Import history.** May import accumulate every earlier source session id in
   `koto/imported-from`? *Assumed, pending state-unification confirmation:* yes.
6. **Read logging.** Decision reads from above write nothing at all, not even to the reader's anchor.
   Counting reads from above, which a later instrument wants, would record them in the reader's own
   session. *Assumed, pending state-unification confirmation:* write nothing now.
