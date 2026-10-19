---
schema: design/v1
status: Planned
problem: |
  koto keeps everything about a workflow inside its session directory, which
  is removed when the workflow succeeds, and its decision-like events carry
  no actor, no identifier another session can point at, and no way to be
  overruled. The decision record needs a host-wide store that outlives
  sessions, identity and authority on every decision, reversal that a live
  workflow honours, and the existing commands writing into it.
decision: |
  Decisions live in a new store at ~/.koto/decisions/: one JSONL file per
  UTC month plus an all-time reversal log and an import log, written through
  one locked writer whose check settles reversal races and import repeats.
  koto decisions grows into the front end (record, list, show, reverse,
  report, import) with shared decider flags. Existing gate-passing commands
  resolve a decider first (handed, else the observed harness session marked
  defaulted, else refuse), write their own session event carrying the whole
  decision, then append to the store, with repair on the next tick. koto
  next blocks on reversed gate passes. Lineage comes from the workspace
  manager's existing session map, labelled handed, through a versioned
  interface a fuller chain can fill later; the acting agent comes from a
  koto-skills hook.
rationale: |
  Plain JSONL with koto's existing lock and repair patterns meets the
  timing budget with no new dependency, and a month-bounded layout plus a
  small reversal log keeps every timed query and the per-tick check cheap.
  Session-first writes keep existing readers unchanged and lose nothing on a
  crash. Reading the workspace manager's records without changing it keeps
  this feature off the dispatch path, and labelling the caller-supplied
  parent as handed keeps the record from claiming more than anyone checked.
upstream: docs/prds/PRD-decision-record.md
decision_provenance: inline-resolved
---

# DESIGN: The decision record

## Status

Planned

## Context and Problem Statement

koto today keeps everything about a workflow inside that workflow's session
directory under `~/.koto/sessions/<name>/`: the append-only session log, the
context keys and the run journal. A session is removed when it reaches a
successful terminal unless `--no-cleanup` is passed, so anything recorded
there is gone by the time a coordinator in another session wants to question
it. The PRD asks for the opposite: every decision a dispatched session makes
or receives must stay readable after the session is removed and its
workspace reclaimed (R17), and any session on the machine must be able to
record and read decisions, including ones about another session's workflow
(R18).

The decision-like events koto already writes don't carry what the PRD needs.
`GateOverrideRecorded` and `DecisionRecorded` keep a rationale and no actor;
`DeciderCheckAnswered` implies the agent; the run journal's `check_overridden`
names a gate and nobody. None has a stable identifier another session could
point at, and none can be reversed: an override lasts for one visit and a
later re-evaluation supersedes it. The PRD needs each decision to carry one
of nine kinds (R1) with a subject a reader can resolve later (R2), an
identifier, a UTC time, an outcome and an optional list of decisions it
relies on (R3).

Who decided is three-way (R4): a session plus the agent within it (`main`,
an agent the main agent started, or `unknown`), a person named by a handle
and the surface it came through (`terminal`, `forge`, `relay`) along with the
session that entered it, or an automated check plus the session that ran it.
koto stores a person's handle as an opaque string and has no account concept
(R5). Every identity is labelled `observed` (koto read it from its
environment or the workspace manager's lineage) or `handed` (the caller
supplied it), and both values are kept when they disagree (R6). koto already
reads `CLAUDE_CODE_SESSION_ID` into the run journal as the driver, which is
the one identity it observes today. It cannot tell a sub-agent's invocation
from the main agent's from its environment: only harness hooks see the agent
identifier.

Authority (R9) is what the decider was dispatched with: an authority kind
(`dispatch` or `person`), the dispatch identifier, a content digest of the
instructions handed down, the run mode and the immediate dispatcher, plus,
on reversals, a `verified` or `asserted` standing. The dispatch chain (R10)
runs from the deciding session to a root that a source affirms; missing
lineage is `incomplete`, never a root. The workspace manager doesn't read or
write workflow state and doesn't run koto (R11). Today it publishes lineage
in two places: each worker's `OTEL_RESOURCE_ATTRIBUTES`
(`niwa.dispatch.id`, `niwa.parent.session.id`), which the agent harness
strips from tool subprocesses before koto could read it, and its session map
at `<root>/.niwa/sessions/<id>.json`, which records the dispatch identifier
and the caller-supplied parent and is deleted at reap. Sessions started by
`niwa watch` get no entry.

Overruling (R12 to R15) is a new decision that points at the original. The
original's chain alone decides who may reverse it; a reversal is final; a
reversed gate pass must stop a still-live workflow from advancing until it is
rewound or the gate is re-passed by someone with standing. Every gate pass by
choice (`koto overrides record`, `koto next --to` past an unmet gate, `koto
decider answer` with a passing answer) must record a `gate-pass` decision;
an unnamed decider defaults to the observed session and is marked
`defaulted`, and a pass with no establishable decider is refused without
writing the command's existing records (R16). The existing commands keep
writing what they write today and also write the decision, together or not
at all (R22, R26); Decision 3 says how that holds without a shared
transaction.

Reading (R19, R20) is by session (made or received), subject, kind, decider
kind and time window, ordered oldest first, plus a report that cross-tabulates
decisions made in a window, and reversals recorded in it, by kind and decider
kind. A record of 100,000 decisions over a year must answer a month's listing
and report in under five seconds (R29). Recording never touches the network
(R28), reasons never reach exported telemetry (R27), writes are append-only
and safe under concurrent writers (R25), and earlier decisions can be
imported idempotently (R24).

koto's request store at `~/.koto/requests/<id>/` is the nearest precedent:
append-only, lock-guarded, written by several sessions, keyed by something
other than a session, and kept after sessions end. It has no index and no
pruning.

## Decision Drivers

- **Outlive the session (R17).** The store can't live under a session
  directory or be removed by any session or workspace cleanup.
- **Cross-session access (R18).** Any session on the host writes and reads,
  under concurrent writers, without losing or corrupting an entry (R25).
- **Query and report speed (R29).** A month out of a year of 100,000
  decisions in under five seconds, and reversal status for any decision
  without a full scan.
- **No dependency on the workspace manager calling koto (R11, R28).** Lineage
  is read from what the workspace manager already keeps, or handed by the
  dispatcher, and recording works when neither is available.
- **Backward compatibility (R26).** Existing events, run-journal readers and
  feed stay unchanged; existing commands keep their arguments.
- **Honest identities (R6, R10).** Every identity says where it came from;
  nothing is guessed.
- **Engine enforcement only where koto can act (R15).** A reversed gate pass
  blocks a live workflow; nothing outside koto is changed.
- **Consistency with koto's conventions.** Atomic writes, append-only logs,
  the name-resolution check over user-facing docs, and the koto-skills
  plugin kept in sync with any CLI change.

## Considered Options

### Decision 1: Where decisions are stored

Delegated to /decision.

Decisions must outlive their session (R17), take writes from any session on
the host (R18) without loss under concurrency (R25), resolve a race between
two reversals of one decision to exactly one winner (R13), make imports
idempotent (R24), and answer a month's listing and the windowed report in
under five seconds over 100,000 decisions (R29). Every view must show whether
a decision is reversed (R14), and `koto next` must find reversed gate passes
on every tick (R15).

Key assumptions:
- Decision lines run 600 to 1,500 bytes; a month is 5 to 12.5 MB and parses
  in well under a second on the CI runner (estimated, not measured).
- Reversals stay a small share of decisions (1 to 10 percent).
- Writes are rare (bursts of tens per second at most), so one store-wide lock
  with a bounded wait never starves.

#### Chosen: Monthly JSONL logs plus an all-time reversal log, under one store lock

A new host-level directory beside the request store, `~/.koto/decisions/`:

- `<YYYY-MM>.jsonl` holds every decision except reversals, in the UTC month
  of its own recorded time. The identifier names the month
  and carries 128 random bits (`d-202610-<32 hex>`), so finding a decision by
  identifier reads one file and collisions are negligible. The writer skips
  an append only when the same identifier with identical content is already
  present; the same identifier with different content is an error.
- `reversals.jsonl` holds every `reverse` decision for all time, each line
  copying the original's identifier, kind, decider kind, deciding session and
  subject key from the same locked read that authorized it. Reversal status,
  the report's reversal table and the per-tick gate check never scan months.
- `imports.jsonl` holds one line per imported source entry (source location
  and entry identifier, the new decision identifier), written before the
  decision, so a repeat import is a lookup.
- `store.lock` is the flock target, with the request store's bounded wait
  (20 ms retries, 5 s deadline, `O_NOFOLLOW`, mode 0600).

Every line is plain JSON with a `v` version field. Every write goes through
one locked writer that repairs a torn tail, runs a check that can append,
skip (already present) or refuse, appends, and syncs. The reversal check runs
under the lock, so the loser of a race sees the winner and is recorded as
`refused`. Reads take no lock, skip a torn final line, and sort by time then
identifier (R19). A month listing or report parses one month file plus the
reversal log. Unwindowed filters use a literal byte search across month files
and parse only matching lines.

#### Alternatives Considered

- **One file per decision in sharded directories with marker files for
  reversals.** Correct and lock-free, but about four times the disk, about
  ten times slower listing, a report that opens thousands of small files, and
  one marker namespace per uniqueness rule where one lock covers them all.
- **Extending the request store.** A reversal of a decision on another
  request needs two locks, which breaks the single-winner check; its append
  re-reads the whole log several times; root-session and person decisions
  have no request to live on.
- **SQLite.** Constraints and indexes would serve every query, but no
  requirement needs them, it would be koto's first non-trivial new crate set,
  and the session log still sits outside its transaction.

### Decision 2: How the dispatch chain reaches koto

Delegated to /decision.

The chain runs from the deciding session to a root a source affirms (R10),
each link labelled `observed` or `handed` (R6), with the dispatch identifier
and a digest of the instructions handed down (R9). The workspace manager must
stay out of workflow state and never run koto (R11), and recording must work
when lineage is missing (R28). The probe that found lineage absent in a
dispatched worker had two causes. The agent harness strips every `OTEL_*`
variable from tool subprocesses, so the telemetry resource attributes never
reach koto. And the workspace manager's session map lives at the workspace
root, not in the instance, where it does hold the dispatch identifier and
parent. That map is deleted at reap, has no instruction digest, can't tell a
missing parent from a malformed one, and isn't written for sessions started
by `niwa watch`.

Key assumptions:
- The harness keeps stripping `OTEL_*` from tool subprocesses; the choice
  doesn't depend on it.
- koto finds the workspace root by walking up from its working directory to
  `.niwa/workspace.toml`; when it can't, the chain is `incomplete`.
- The session map's `dispatch_id` and `parent_session_id` keep their current
  names and meaning. The parent is the dispatching caller's own claim, which
  the workspace manager records without verifying it.

#### Chosen: Read the workspace manager's existing session map as handed lineage, through a versioned lineage interface that a richer chain can fill later

koto reads only what the workspace manager already writes, and stays off its
dispatch path:

1. koto reads `CLAUDE_CODE_SESSION_ID`, finds the workspace root, and reads
   `<root>/.niwa/sessions/<id>.json` from disk. It never runs `niwa` and
   never writes there.
2. From today's entry it takes `dispatch_id` and `parent_session_id`, labelled
   `handed`: the workspace manager records the parent as the caller supplied
   it, so koto passes that provenance on rather than upgrading it. It walks
   parent by parent through the entries that still exist. An entry with no
   parent ends the chain as `incomplete`, because today's map can't say
   whether the session was a root or its parent was simply not recorded.
3. koto defines a versioned lineage interface it reads when an entry carries
   one: an object `lineage` with `v`, `dispatch_id`, `brief_digest`, `parent`
   as `session:<id>`, `none` or `unknown`, an optional copied `chain` of such
   links, and a per-field `source` of `observed` or `handed`. koto takes each
   value with the label the interface gives it, treats `parent: none` as an
   affirmed root, and treats an unknown version as absent.
4. The first time koto sees a driver, it stamps the lineage it resolved into
   its run-journal record for that driver, so a session resumed after its
   entry is reaped still resolves. Every decision stores a snapshot of its
   resolved chain, dispatch identifier and digest.
5. Handed values also arrive on commands that record decisions:
   `--dispatched-by session:<id>|person:<handle>|none`, `--chain-file
   <path>` (links above the dispatcher, in the same lineage format),
   `--instructions-digest sha256:<hex>`, and `--dispatch-id`. They sit in
   their own slot per link, and a disagreement with another source marks the
   chain `incomplete`.

Three workspace manager changes would fill the interface. They belong to the
planned work on the recorded dispatch tree, not to this feature, and this
design consumes them once they land without needing them to ship:

- every session entry carries the versioned `lineage` object above, with the
  instruction digest the workspace manager already computes and an explicit
  `parent: none` for a dispatch with no caller session;
- at dispatch, the dispatching session's chain is copied into the child's
  entry while every ancestor still exists, so a reaped ancestor leaves no gap;
- sessions started by `niwa watch` get an entry, with `parent: none`.

Until then, chains read from the workspace manager end `incomplete` and carry
`handed` labels, and a root is affirmed only by a handed `--dispatched-by
none` or a chain file.

Handed values never raise a reversal's standing: a reversal earns `verified`
only when the matching link is `observed` (the original decider's own
observed session, for example), and is `asserted` when the match rests on
handed links alone.

#### Alternatives Considered

- **koto parses the telemetry resource attributes.** The harness strips them
  before koto runs.
- **Plain environment variables.** Environment holds only what the parent's
  environment held, is lost on a plain resume, and its delivery to tool
  subprocesses is unmeasured.
- **Require the workspace manager changes before this feature ships.** Would
  put this feature on the dispatch path and behind another feature's
  delivery, when the record is useful with incomplete chains.
- **Label the session map's parent `observed`.** koto does read it from disk
  itself, but the value is the dispatcher's claim. Upgrading it would make
  the record say more than anyone checked.
- **Handed only, ignoring the session map.** Throws away the dispatch
  identifier and parent the workspace manager already records.

### Decision 3: The command surface and how existing commands write decisions

Resolved inline.

The PRD needs commands to record, show, list, report, reverse and import
decisions (R19, R20, R24), and the three gate-passing commands plus `koto
decisions record` must write a decision alongside what they write today,
together or not at all (R16, R22, R26).

Key assumptions:
- The session log can't share a transaction with the store, so "together"
  means ordered writes plus repair.

#### Chosen: Extend `koto decisions`; session event first, store second, repair on the next tick

`koto decisions record` keeps its current form (`<name> --with-data`) and maps
it to a `judgment`; new flags record any kind and subject. `koto decisions
list` keeps its per-workflow form and gains host-wide filters. New
subcommands `show <id>`, `reverse <id>`, `report` and `import <source>`
complete the surface. One set of decider flags is shared by every recording
command, all optional, so no existing call breaks.

Each existing command resolves the decider first (Decision 4), generates the
decision identifier, writes its own session event with the identifier and the
full decision, then appends to the store, skipping if the identifier is
already there. If the store append is lost, the next `koto next` on that
session finds an event naming a decision the store lacks and re-appends the
identical decision. A refused pass writes only the `refused` decision.

#### Alternatives Considered

- **A new noun (`koto record`).** Two names for one concept, next to `koto
  decisions`, which would then mean something narrower.
- **Folding decisions into `koto request`.** Root sessions have no request.
- **Store first, voiding on a failed session write.** Needs a void marker
  every reader honours; superseded by the repair model, which loses nothing
  because the session event carries the whole decision.

### Decision 4: Enforcing a reversed gate pass and the decider requirement

Resolved inline.

A reversed gate pass must stop a still-live workflow at whatever state it has
reached until it is rewound to or before the gated state, or the gate is
re-passed by the reverser or a decider with standing over the reverser (R15).
A gate pass with no establishable decider must be refused before anything is
written (R16).

#### Chosen: One decider resolver before any write; a `decision_reversed` blocking condition at `koto next`

The three gate-passing commands share a resolver: a handed `--by` wins;
otherwise the observed `CLAUDE_CODE_SESSION_ID`, marked `defaulted`;
otherwise exit non-zero with `decider_missing`, write none of the command's
records, and record a `refused` decision. The resolver runs before the gate
is touched. Once a team can declare who may decide at a gate, the resolver
also checks the resolved decider against that declaration and refuses a pass
by anyone else (R16a); until declarations exist, that check finds no
declared deciders and changes nothing, so no code for it is written now.
People passing a gate from inside an agent's session are told, in
koto-user, to pass `--by person:<handle>`.

`koto next` already loads the session's own gate-pass decision identifiers,
rewinds and re-passes from its events. Beside the cancelled-workflow check in
`handle_next`, it asks the reversal log which of those identifiers are
reversed (sessions with none open nothing). For each reversed pass not cleared
by a later rewind to or before the gated state, or by a later qualifying
`gate-pass` for the same gate, it returns a blocking condition of category
`decision_reversed` naming the reversal, and refuses every transition.

#### Alternatives Considered

- **Writing into the original session at reversal time.** The session may be
  gone or locked, and writing into another session's log is a cross-session
  write path koto doesn't have.
- **Forcing a rewind.** The PRD says koto doesn't move the workflow back
  itself.

### Decision 5: Capturing the acting agent

Resolved inline.

A `session` decider names the agent within the session that decided: `main`,
an agent the main agent started, or `unknown` (R4). koto's environment is the
same for the main agent and the agents it starts; only harness hooks receive
the acting agent's identifier.

Key assumptions:
- The harness's PreToolUse payload carries the sub-agent identifier for calls
  made inside a sub-agent and lets a hook rewrite the command.

#### Chosen: A koto-skills hook sets `KOTO_AGENT_ID`; `--agent` is the handed fallback

The koto-skills plugin ships a PreToolUse hook that, for Bash calls invoking
`koto`, prefixes `KOTO_AGENT_ID=<id>` (quoted, and only when the value
matches koto's identifier pattern) from the payload's agent identifier. It
sets `main` only when the payload positively identifies the main agent, and
otherwise sets nothing, so an ambiguous call records `unknown` rather than
over-claiming. koto reads the variable as `observed`; `--agent` is `handed`;
both are kept when they differ; neither means `unknown`. A command that
reassigns the variable or wraps koto in another shell can still defeat the
hook; the label says the value came from the environment, not that it was
authenticated.

#### Alternatives Considered

- **Flag only.** Agents rarely know their own identifier and nothing checks
  it.
- **Session only.** Always `unknown`, which empties the session-and-agent
  distinction in practice.

### Decision 6: Smaller choices made while writing the architecture

Resolved inline.

- **Store location follows the request store.** The store sits at
  `$HOME/.koto/decisions/`, resolved the way the request store resolves
  `$HOME/.koto/requests/`, so tests isolate it by setting `HOME` as they
  already do. Following `KOTO_SESSIONS_BASE` instead was rejected: that
  variable moves session directories, and the decision store must not move
  with them, because decisions outlive sessions.
- **No wake on reversal in the first version.** A worker parked in `koto
  request watch` notices a reversed gate pass at its next `koto next`, not
  immediately. Ringing its wake file from the reversing session was
  considered and deferred: it adds a second cross-session write and no
  requirement asks for immediacy.
- **Reasons stay out of the run journal.** The run journal gains decision
  identifiers and kinds, never reasons, matching its existing rule for
  override rationales (R27). Carrying the reason was rejected because the
  run journal is what telemetry exporters read.

## Decision Outcome

The five choices form one path. A decision starts at a recording command (a
new `koto decisions` subcommand or one of the existing gate-passing
commands). koto resolves the decider: a handed `--by`, or the observed
harness session marked `defaulted`, with the agent from the koto-skills hook
or `--agent`, or a refusal when nothing can be established. It resolves the
authority and chain from the workspace manager's session entry and its own
run-journal stamps, takes any handed lineage into a separate slot, and
snapshots the result onto the decision. The decision then lands in the
host-level store: a month file for most kinds, the reversal log for
reversals, under one lock whose check settles reversal races and import
repeats. Existing commands write their own session event first, carrying the
whole decision, and a lost store append is repaired on the next tick. Reading
is lock-free over one month file plus the reversal log. `koto next` consults
the reversal log for the session's own gate passes and blocks when one has
been reversed and not cleared. The workspace manager is unchanged: koto reads
its existing session map, and a versioned lineage interface lets later
workspace manager work supply a fuller chain.

## Solution Architecture

### Overview

A new module owns the decision store and everything that reads or writes it.
The CLI's `koto decisions` noun becomes its front end. Four existing commands
call into it as writers, `koto next` calls into it to enforce reversals and
repair lost appends, and a lineage resolver turns what koto observes
(harness session, hook-set agent, workspace manager session entries, its own
run-journal stamps) plus anything handed into the identity, authority and
chain snapshot each decision carries.

### Components

- **`src/decision_store/`** (new): the record format, the locked writer
  (lock, torn-tail repair, check closure that appends, skips or refuses,
  append, `sync_data`), lock-free readers that sort by time then identifier,
  the reversal index reads, the report aggregator and the importer.
- **`src/decision_store/lineage.rs`** (new): resolves the deciding session,
  agent, authority and chain, labelling every value `observed` or `handed`
  and marking chains `incomplete` per R10. Reads `CLAUDE_CODE_SESSION_ID`,
  `KOTO_AGENT_ID`, the workspace manager's session entries, and the run
  journal's driver stamps.
- **`src/decision_store/decider.rs`** (new): the decider resolver shared by
  every recording command (handed `--by`, else the observed session marked
  `defaulted`, else `decider_missing`), and the agent read from
  `KOTO_AGENT_ID` or `--agent`.
- **`src/decision_store/authority.rs`** (new): R12's reversal rules over two
  recorded chains, returning `verified`, `asserted` or a refusal reason.
- **`src/cli/decisions.rs`** (extended from the current handler): `record`,
  `list`, `show`, `reverse`, `report`, `import`, and the shared decider and
  handed-lineage flags.
- **Existing writers**: `src/cli/overrides.rs`, the directed-transition path
  in `koto next --to`, and `src/cli/decider_answer.rs` call the decider
  resolver before any write and dual-write per Decision 3. `koto decisions
  record` in its current form writes a `judgment`.
- **Engine** (`handle_next`): the reversal check beside the cancelled-workflow
  check, the `decision_reversed` blocking condition, and the repair of
  decisions named in session events but missing from the store.
- **Run journal**: the driver record gains the observed lineage stamp; the
  existing override and answer records gain the decision identifier.
- **koto-skills plugin**: the PreToolUse hook that sets `KOTO_AGENT_ID`, and
  updated `koto-user`, `koto-author` and `koto-adhoc` documentation for the
  new flags and subcommands.
- **Workspace manager**: no change in this feature. koto reads its existing
  session map, and the lineage interface defines what later workspace
  manager work can add.
- **shirabe plugin** (separate repository): the findings ledger writing
  `verdict` and `judgment` decisions, the coordinator's decision table
  rendered from `koto decisions list`, and document acceptance and merges
  writing `accept` and `merge` decisions.

### Key Interfaces

A decision line (one JSON object, field names indicative):

```json
{
  "v": 1,
  "id": "d-202610-3f9a0c1e5b7d42a8a61c0e9f2b4d7e11",
  "time": "2026-10-19T17:04:11Z",
  "kind": "gate-pass",
  "outcome": "passed",
  "subject": {"workflow": "work-on-123", "workflow_session_id": "s-...", "state": "review", "gate": "panel"},
  "decided_by": {
    "kind": "session",
    "session": [{"value": "9c1e...", "source": "observed"}],
    "agent": [{"value": "main", "source": "observed"}],
    "defaulted": true
  },
  "authority": {
    "kind": "dispatch",
    "dispatch_id": [{"value": "disp-...", "source": "handed"}],
    "instructions_digest": [],
    "run_mode": [{"value": "unattended", "source": "observed"}],
    "dispatcher": [{"value": "session:4b2d...", "source": "handed"}]
  },
  "chain": {"links": [[{"value": "session:4b2d...", "source": "handed"}]], "incomplete": true},
  "basis": [],
  "evidence": ["journal:work-on-123#42"],
  "reason": "panel finding is a false positive"
}
```

Every identity, and every chain link, has one shape: a list of values each
with its `source`. One entry is the usual case; two entries mean an observed
and a handed value disagreed (R6); an empty list means nothing was found.
Standing is computed from these labels when a reversal is checked.

A reversal line in `reversals.jsonl` adds `reverses` (the original's
identifier) and `original` (its kind, decider kind, deciding session and
subject key), and its authority carries `standing: verified|asserted`. A
`person` decider carries `handle` (`handed`), `surface`, an optional `link`
and `entered_by` (a session). A `check` decider carries `name` and `ran_in`
(a session). A decision re-appended by the repair tick carries
`repaired_from` (the session whose event supplied it); an imported one
carries `import_source` and keeps the source's time in `time` alongside its
own `recorded_at`.

Session events: `GateOverrideRecorded`, `DirectedTransition`,
`DeciderCheckAnswered` and `DecisionRecorded` gain one optional field,
`decision`, holding the full decision line the command is about to append.
Existing readers ignore it, because no payload struct denies unknown fields.
The repair tick scans the session's own events for `decision` fields whose
identifier the store lacks and appends them through the locked writer. It
never re-appends a `reverse` line, so a forged session event can't bypass
R12 and R13, and it runs before a terminal tick's cleanup, so the last
decision of a run is repaired before its session is removed.

Commands (flags indicative; names resolve against the clap tree once built):

| Command | Purpose |
|---|---|
| `koto decisions record --kind <k> --outcome <o> <subject flags> [decider flags]` | record any kind (R1 to R3) |
| `koto decisions record <name> --with-data <json>` | today's form, recorded as `judgment` |
| `koto decisions list [<name>] [--session <id> [--received]] [--subject ...] [--kind ...] [--decider-kind ...] [--run-mode ...] [--dispatch-id ...] [--instructions-digest ...] [--defaulted] [--since ...] [--until ...]` | filtered listing, oldest first, each with in-effect status (R19) |
| `koto decisions show <id>` | one decision, its reversal and its affected `basis` dependents (R15) |
| `koto decisions reverse <id> [--compensating <url>] [--replacement <outcome>] [decider flags]` | record a reversal subject to R12 and R13; `--compensating` takes a revert pull request for a merge or a rollback for a release verdict |
| `koto decisions report --since <t> --until <t>` | the R20 tables, the count of defaulted gate passes, and the list |
| `koto decisions import <source>` | idempotent import (R24), first format the coordinator's decision table |

Shared decider flags: `--by session|person:<handle>|check:<name>`,
`--surface terminal|forge|relay`, `--link <https-url>`, `--agent <id|main>`,
`--run-mode attended|unattended`, `--basis
<id>` (repeatable), `--evidence <pointer>` (repeatable), `--reason <text>`.
Subject flags include `--repo` and `--commit` (required on a `verdict` about
a change or release) and, for intent acceptance, `--intent-location` and
`--source-link`. When the harness sets `CLAUDE_CODE_SESSION_ATTENDED`, koto
records the run mode from it as `observed`; `--run-mode` is the handed
fallback.
Handed lineage flags: `--dispatched-by`, `--chain-file`,
`--instructions-digest`, `--dispatch-id`. All output is JSON.

The workspace manager's `lineage` object (in `<root>/.niwa/sessions/<id>.json`):

```json
{"lineage": {"v": 1, "dispatch_id": "...", "brief_digest": "sha256:...",
  "parent": "session:<id>|none|unknown",
  "chain": [{"session": "<id>", "dispatch_id": "...", "parent": "session:<id>|none|unknown"}],
  "origin": "dispatch|watch"}}
```

### Data Flow

1. A recording command runs. The lineage resolver reads the harness session,
   the hook-set agent and the workspace manager's entry for the session (and
   the run-journal stamp as fallback), merges handed flags into their own
   slots, and resolves the chain.
2. For an existing gate-passing command, the decider resolver either
   produces a decider or refuses. On refusal the store gets one `refused`
   line and the command exits non-zero.
3. The command writes its session event with the decision identifier and the
   full decision, then calls the store's locked writer, which appends the
   line to the month file (or, for `reverse`, checks R12 and R13 under the
   lock and appends to `reversals.jsonl` or appends a `refused` line).
4. Readers (`list`, `show`, `report`) read the month files they need plus the
   reversal log, without a lock, and join reversal status in memory.
5. On each `koto next`, the engine re-appends any decision its session events
   name that the store lacks, then asks the reversal log about the session's
   own gate passes, and blocks on any reversed pass not cleared.

## Implementation Approach

### Phase 1: The store and its readers

The record format, the locked writer with its check closure, the readers,
`show` and `list` filters, `report` and the R29 performance fixture. The
writer's refuse-and-skip behaviour is tested with a stub check closure here;
the real race tests arrive with the operations that race.

Deliverables:
- `src/decision_store/` (format, writer, readers, report)
- `koto decisions record --kind`, `show`, `list` filters, `report`
- Torn-tail, identifier-collision, stub-closure and performance tests

### Phase 2: Identity, authority and lineage

The decider resolver, the lineage resolver against the workspace manager's
current session entries (dispatch identifier and parent, labelled handed) and
the versioned lineage interface when present, the run-journal driver stamp,
handed lineage flags, the agent variable, and R6 and R10 labelling, tested
against fixture entries of both shapes.

Deliverables:
- `src/decision_store/decider.rs`, `src/decision_store/lineage.rs`
- Run-journal driver stamp extension
- Unit tests for every label, `defaulted` and `incomplete` case

### Phase 3: Reversal

`koto decisions reverse`, R12's rules and standing, R13's refusals, per-kind
effects (affected `basis` dependents, compensating link, replacement
outcome), and the `decision_reversed` blocking condition in `handle_next`,
with its clearing rule reusing the R12 function.

Deliverables:
- `src/decision_store/authority.rs`
- Engine check and blocking condition
- Functional tests for each R15 case, clearing by a qualifying decider and
  refusal to clear by an unrelated one, and the race test of two reversals
  of one decision

### Phase 4: Existing commands, import and repair

The decider resolver and dual-write in `koto overrides record`, `koto next
--to` and `koto decider answer`; today's `koto decisions record` writing a
`judgment`; the `decision` field on session events and the repair tick;
run-journal records gaining the decision identifier; `koto decisions import`
for the coordinator's table.

Deliverables:
- Changes in `src/cli/overrides.rs`, `src/cli/decider_answer.rs`, the `--to`
  path, `src/cli/decisions.rs`, and the session event payloads
- Import, with the race test of a reversal racing an import of the same
  entry
- Existing functional tests updated only to set a harness session
  identifier where they pass gates

### Phase 5: koto-skills

The PreToolUse hook, and `koto-user`, `koto-author` and `koto-adhoc` updates
with evals. The hook can be built alongside Phase 2; the documentation lands
with Phase 4.

Deliverables:
- Hook script and plugin manifest entry
- Skill updates and eval results

### Phase 6: shirabe (separate repository)

The findings ledger, coordinator table view, document acceptance and merge
writers in the shirabe plugin, and the disputes change adopting the R4
vocabulary. These depend on Phases 1 to 4 being released; each shirabe
writer checks for `koto decisions record --kind` and keeps its current
behaviour against an older koto.

Deliverables:
- shirabe: decision writers and the coordinator table view

## Security Considerations

The decision record is attribution, not authorization. koto stores who claims to have decided, what it observed in its environment and the workspace manager's files, and what the caller handed it. Anything running as the same user can set `CLAUDE_CODE_SESSION_ID` or `KOTO_AGENT_ID`, edit a session entry, or write the store directly, so `observed` means koto read the value from its environment or from files, not that anyone authenticated it. Standing on a reversal works the same way: `verified` means the reversal's chain agrees with the original's on observed links, and `asserted` means it agreed only on links the caller handed in. Neither is a permission check. The `decision_reversed` block in `koto next` is an engine safeguard for honest disputes and mistakes. Any reversal in effect blocks, whatever its standing, because a forged reversal can only stop a workflow, never pass a gate, so the failure is safe. Clearing a block needs the same standing the reversal had: a `verified` reversal is cleared only by a rewind or by a `gate-pass` from a decider whose R12 match over the reverser is `verified`, so an `asserted` claim can't undo it. A mistaken or forged reversal is recovered the same way, by the reverser, someone above it, or a rewind. The blocking condition carries only the reversal's identifier, kind, standing and time; its reason and handle are shown by `koto decisions show`, marked as quoted data, so a reversal can't put text in front of a running worker. Merges, releases and anything else that needs real authorization must be enforced by the forge or the workspace manager.

Handed lineage can't raise standing. Handed values live in their own slot on each link and are never merged into the observed slot. A handed value that disagrees with an observed one marks the chain `incomplete`, and so does a handed `none` against an observed parent. An `incomplete` chain is never eligible for `verified`. Reports and listings show handed values separately from observed ones and don't count handed-only authority as observed.

koto validates every identifier it turns into a path or a command. Session ids, dispatch ids and parent ids from the environment or from session entries must match a fixed pattern (letters, digits, `.`, `_`, `-`, bounded length) before koto builds `<root>/.niwa/sessions/<id>.json`, and the parent-by-parent walk stops at a fixed depth and at the first repeated id. `KOTO_AGENT_ID` is held to the same pattern, and a value that fails it is read as `unknown`. The hook that sets it quotes the value. koto reads a workspace manager entry only if, checked with `fstat` on the opened descriptor, it is a regular file owned by the current user that no other user can write, and treats it as absent otherwise. A planted `.niwa/` tree in an untrusted checkout can at most supply `handed` lineage. `--chain-file` and `import` sources are opened without following symlinks, must be regular files, are size-capped and parsed as strict versioned JSON before koto takes the store lock. Everything taken from an import source is recorded as handed, with the source location.

The store is private to the user. `~/.koto/decisions/` is created with mode 0700 and koto refuses to use it if it's owned by someone else or writable by group or others. The month files, `reversals.jsonl`, `imports.jsonl` and the lock are opened with mode 0600 and `O_NOFOLLOW`, and torn-tail repair truncates the opened file descriptor after confirming it's a regular file, never a path. On a shared host the directory mode is the only barrier, and every session of the same user can read every decision, including ones about unrelated projects.

Line and field sizes are capped. A decision line may not exceed a fixed size (64 KiB), repeated flags (`--basis`, `--evidence`) and chain depth have fixed maxima, and koto refuses an oversized record with an error instead of truncating it, so one write can't slow every reader or the per-tick reversal lookup. Readers of month files skip a malformed or unknown-version line and report how many they skipped. The reversal log fails closed: if `koto next` meets a line in `reversals.jsonl` it can't read, it returns a `decision_store_unreadable` blocking condition rather than treating any pass as not reversed. Torn-tail repair appends the missing newline when the last line is complete JSON, and truncates only a line that doesn't parse. All inputs are read, parsed and assembled before the store lock is taken, so the lock is held only for the check, the append and the sync. A lock timeout during the repair tick fails neither the tick nor a gate pass: the tick retries on the next one, and a timeout never turns into a passed gate.

The repair on `koto next` appends only decisions named in session events that pass koto's usual event validation, whose identifier matches its own time's month and isn't already in the store, and marks them as repaired with the session that supplied them, so a forged session event can't pass as a direct recording.

What's stored is permanent and has no delete path: handles, reasons, links and evidence pointers. Reasons, links, handles, evidence pointers, compensating URLs, chain-file contents and imported fields all go through koto's redaction and the field caps before they're written, `--link` must be `https` with no credentials or query string, and the documentation says not to put secrets in reasons. Handles are opaque strings the caller chooses; koto doesn't validate or resolve them. Reasons, handles and links are data: consumers such as the coordinator's table and the shirabe skills must not read instructions into them. Nothing under `~/.koto/decisions/` is read by any telemetry exporter, and the run journal receives decision identifiers and kinds only, never reasons, handles or links. Backups or syncs that copy the user's `~/.koto/` copy this history with them. Scrubbing a leaked secret means stopping writers, taking the lock and rewriting the file by hand, and the documentation describes that procedure.

No new crates are added, so the design adds no dependency risk. It depends on the workspace manager's session map and on the koto-skills hook, and trusts them no more than it trusts the file permissions around them.

## Consequences

### Positive

- One record answers who decided, under what authority and whether it still
  stands, for every decision kind, after the deciding session is gone.
- A reversed gate pass actually stops a live workflow, so overruling is more
  than a note.
- The store is plain JSONL with koto's existing lock and repair patterns, adds
  no dependency, and can be inspected with text tools.
- Existing commands, events, run-journal readers and the feed keep working;
  the new fields are additive.
- The workspace manager stays out of workflow state: it writes facts about
  sessions it launched into its own files.

### Negative

- Unwindowed queries are linear in the size of the record, and the record
  grows until pruning exists.
- Correctness for reversal races and import idempotency rests on every write
  going through one locked writer.
- "Together or not at all" for existing commands is ordered writes plus a
  repair on the next tick, so a decision can be missing from the store
  between a crash and that tick.
- Agent identity depends on a hook only one harness provides; elsewhere it is
  `unknown`.
- Until the workspace manager publishes the lineage interface, chains read
  from it carry `handed` labels and end `incomplete`, so reversals by a
  coordinator are `asserted` unless lineage is handed explicitly.
- The record is per host; a session resumed on another host doesn't see
  reversals recorded on the first.

### Mitigations

- Month sharding keeps the timed queries bounded, and a derived index can be
  added later without changing the record.
- The two race tests are acceptance tests for Phases 3 and 4.
- The repair runs on every `koto next`, and the session event carries the
  whole decision, so nothing is lost.
- `unknown` and `incomplete` are recorded honestly, so consumers can tell
  missing attribution from present attribution.
- The cross-host limitation is documented alongside the command reference.
