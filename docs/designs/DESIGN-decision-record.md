---
status: Proposed
problem: |
  koto has no durable, cross-session record of the decisions dispatched
  sessions make. Decision-like events live in session logs that are deleted
  on success, carry no actor, and can't be overruled from another session.
decision: |
  To be settled in this design.
rationale: |
  To be settled in this design.
upstream: docs/prds/PRD-decision-record.md
---

# DESIGN: The decision record

## Status

Proposed

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
only in two places: each worker's `OTEL_RESOURCE_ATTRIBUTES`
(`niwa.dispatch.id`, `niwa.parent.session.id`) and its per-instance session
mapping, which it deletes at reap. A check run inside a dispatched worker
for this design found neither present, so neither can be relied on in every
launch path as things stand.

Overruling (R12 to R15) is a new decision that points at the original. The
original's chain alone decides who may reverse it; a reversal is final; a
reversed gate pass must stop a still-live workflow from advancing until it is
rewound or the gate is re-passed by someone with standing. Every gate pass by
choice (`koto overrides record`, `koto next --to` past an unmet gate, `koto
decider answer` with a passing answer) must record a `gate-pass` decision;
an unnamed decider defaults to the observed session and is marked
`defaulted`, and a pass with no establishable decider is refused without
writing the command's existing records (R16). The existing commands keep
writing what they write today and also write the decision, atomically (R22,
R26).

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
  (`d-202610-<random>`), so finding a decision by identifier reads one file.
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
- The workspace manager agrees to publish the versioned lineage object below,
  and to write entries for watch launches.

#### Chosen: Observed lineage from the workspace manager's session entry, chain copied at dispatch, snapshotted per decision, handed values as a separate fallback

On the workspace manager's side, all file writes it already knows how to
make, none touching workflow state:

1. Every session entry under `<root>/.niwa/sessions/<id>.json` carries a
   versioned `lineage` object: `dispatch_id`, `brief_digest` (the `sha256:`
   digest it already computes), and `parent` as `session:<id>`, `none`
   (dispatched from a shell with no caller session, which affirms a root) or
   `unknown`.
2. At dispatch, while every ancestor's entry still exists, it copies the
   dispatching session's chain into the child's entry, so a reaped ancestor
   leaves no gap.
3. `niwa watch` writes the same entry for each session it launches, with
   `parent: none` and the trigger as an opaque string.
4. The format is documented as a contract other tools read by name.

On koto's side:

1. koto reads `CLAUDE_CODE_SESSION_ID`, finds the workspace root, and reads
   that session's entry from disk, ignoring unknown fields and treating an
   unknown version as absent. It never runs `niwa`.
2. The session's own link comes from the entry, then koto's own run-journal
   stamp for that driver. Ancestor links come from the copied chain, then a
   parent-by-parent walk of root entries, then ancestors' run-journal stamps.
   All of these are `observed`.
3. A link is the root only when an observed `parent: none` says so, or a
   handed root closes a chain no observed link goes past. `parent: unknown`,
   a missing entry or an unreadable one ends the chain as `incomplete`.
4. The first time koto sees a driver, it stamps the observed lineage into its
   run-journal record for that driver, so a session resumed after its entry
   is reaped still resolves.
5. Every decision stores a snapshot of its resolved chain, dispatch
   identifier and digest, so a later reap can't change what it says.
6. Handed values arrive only on commands that record decisions:
   `--dispatched-by session:<id>|person:<handle>|none`, `--chain-file
   <path>` (links above the dispatcher, in a JSON format koto owns),
   `--instructions-digest sha256:<hex>`, and `--dispatch-id` for
   orchestrators other than this workspace manager. Handed values sit in
   their own slot per link, are never merged into the observed slot, and a
   disagreement marks the chain `incomplete`.

A reversal that matches the original's chain only on a handed link gets
standing `asserted`, never `verified`.

#### Alternatives Considered

- **koto parses the telemetry resource attributes.** The harness strips them
  before koto runs.
- **Plain environment variables as the primary channel.** Environment holds
  only what the parent's environment held, breaks at any hop launched by
  older tooling or a person, is lost on a plain resume, and its delivery to
  tool subprocesses is unmeasured. Kept as a possible later cross-check for
  the session's own link, gated on a live test.
- **A new reap-proof lineage log owned by the workspace manager.**
  Unnecessary once the chain is copied into each child's entry at dispatch.
- **Handed only.** Watch sessions have no dispatcher to hand anything, and
  reversal refusals would rest on the dispatching skills' honesty.

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
is touched.

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
`koto`, prefixes `KOTO_AGENT_ID=<id>` from the payload, or `KOTO_AGENT_ID=main`
when the payload names no sub-agent. koto reads it as `observed`; `--agent` is
`handed`; both are kept when they differ; neither means `unknown`.

#### Alternatives Considered

- **Flag only.** Agents rarely know their own identifier and nothing checks
  it.
- **Session only.** Always `unknown`, which empties the session-and-agent
  distinction in practice.

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
been reversed and not cleared. The workspace manager's part is limited to
writing a versioned lineage object, and a copied chain, into session entries
it already writes.

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
- **Workspace manager** (separate repository): the versioned `lineage`
  object, the chain copy at dispatch, and entries for watch launches.
- **shirabe plugin** (separate repository): the findings ledger writing
  `verdict` and `judgment` decisions, the coordinator's decision table
  rendered from `koto decisions list`, and document acceptance and merges
  writing `accept` and `merge` decisions.

### Key Interfaces

A decision line (one JSON object, field names indicative):

```json
{
  "v": 1,
  "id": "d-202610-7f3a9c",
  "time": "2026-10-19T17:04:11Z",
  "kind": "gate-pass",
  "outcome": "passed",
  "subject": {"workflow": "work-on-123", "workflow_session_id": "s-...", "state": "review", "gate": "panel"},
  "decided_by": {
    "kind": "session",
    "session": {"observed": "9c1e...", "handed": null},
    "agent": {"observed": "main", "handed": null},
    "defaulted": true
  },
  "authority": {
    "kind": "dispatch",
    "dispatch_id": {"observed": "disp-...", "handed": null},
    "instructions_digest": {"observed": "sha256:...", "handed": null},
    "run_mode": "unattended",
    "dispatcher": {"observed": "session:4b2d...", "handed": null}
  },
  "chain": {"links": [{"session": "...", "source": "observed"}, {"root": "none", "source": "observed"}], "incomplete": false},
  "basis": [],
  "evidence": ["journal:work-on-123#42"],
  "reason": "panel finding is a false positive",
  "confidence": "medium"
}
```

A reversal line in `reversals.jsonl` adds `reverses` (the original's
identifier) and `original` (its kind, decider kind, deciding session and
subject key), and its authority carries `standing: verified|asserted`. A
`person` decider carries `handle` (`handed`), `surface`, an optional `link`
and `entered_by` (a session). A `check` decider carries `name` and `ran_in`
(a session).

Commands (flags indicative; names resolve against the clap tree once built):

| Command | Purpose |
|---|---|
| `koto decisions record --kind <k> --outcome <o> <subject flags> [decider flags]` | record any kind (R1 to R3) |
| `koto decisions record <name> --with-data <json>` | today's form, recorded as `judgment` |
| `koto decisions list [<name>] [--session <id> [--received]] [--subject ...] [--kind ...] [--decider-kind ...] [--since ...] [--until ...]` | filtered listing, oldest first, each with in-effect status (R19) |
| `koto decisions show <id>` | one decision, its reversal and its affected `basis` dependents (R15) |
| `koto decisions reverse <id> [--compensating <url>] [--replacement <outcome>] [decider flags]` | record a reversal subject to R12 and R13 |
| `koto decisions report --since <t> --until <t>` | the R20 tables and list |
| `koto decisions import <source>` | idempotent import (R24), first format the coordinator's decision table |

Shared decider flags: `--by session|person:<handle>|check:<name>`,
`--surface terminal|forge|relay`, `--link <https-url>`, `--agent <id|main>`,
`--run-mode attended|unattended`, `--confidence high|medium|low`, `--basis
<id>` (repeatable), `--evidence <pointer>` (repeatable), `--reason <text>`.
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

### Phase 1: The store

The record format, the locked writer, readers, `show` and `list` over the new
store, with the two race tests (two reversals racing, a reversal racing an
import of the same entry) as acceptance tests.

Deliverables:
- `src/decision_store/` (format, writer, readers)
- `koto decisions record --kind`, `show`, `list` filters
- Race and torn-tail tests

### Phase 2: Identity, authority and lineage

The lineage resolver against the workspace manager's current session entries
(which already carry the parent and dispatch identifier), the run-journal
driver stamp, handed lineage flags, the agent variable, and R6 and R10
labelling. Works before the workspace manager change lands, with chains that
end `incomplete` where the entry lacks a root marker.

Deliverables:
- `src/decision_store/lineage.rs`
- Run-journal driver stamp extension
- Unit tests for every label and `incomplete` case

### Phase 3: Reversal

`koto decisions reverse`, R12's rules, R13's refusals, per-kind effects
(affected `basis` dependents, compensating link, replacement outcome), the
`decision_reversed` blocking condition in `handle_next`, and the store repair
tick.

Deliverables:
- `src/decision_store/authority.rs`
- Engine check and blocking condition
- Functional tests for each R15 case

### Phase 4: Existing commands and the report

The decider resolver and dual-write in `koto overrides record`, `koto next
--to` and `koto decider answer`; today's `koto decisions record` writing a
`judgment`; run-journal records gaining the decision identifier; `koto
decisions report`; `koto decisions import` for the coordinator's table; the
performance fixture for R29.

Deliverables:
- Changes in `src/cli/overrides.rs`, `src/cli/decider_answer.rs`, the `--to`
  path, `src/cli/decisions.rs`
- Report and import
- Existing functional tests updated only to set a harness session
  identifier where they pass gates

### Phase 5: koto-skills

The PreToolUse hook, and `koto-user`, `koto-author` and `koto-adhoc` updates
with evals.

Deliverables:
- Hook script and plugin manifest entry
- Skill updates and eval results

### Phase 6: Workspace manager and shirabe (separate repositories)

The versioned lineage object, chain copy and watch entries in the workspace
manager; the findings ledger, coordinator table view, document acceptance and
merge writers in the shirabe plugin, and the disputes change adopting the R4
vocabulary. These depend on Phases 1 to 4 being released.

Deliverables:
- Workspace manager: lineage object, chain copy, watch entries, documented
  format
- shirabe: decision writers and the coordinator table view

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
- Observed lineage depends on a change in the workspace manager and on koto
  finding the workspace root. Until then, chains often end `incomplete` and
  reversals are `asserted` more often than `verified`.
- The record is per host; a session resumed on another host doesn't see
  reversals recorded on the first.

### Mitigations

- Month sharding keeps the timed queries bounded, and a derived index can be
  added later without changing the record.
- The two race tests are acceptance tests for Phase 1.
- The repair runs on every `koto next`, and the session event carries the
  whole decision, so nothing is lost.
- `unknown` and `incomplete` are recorded honestly, so consumers can tell
  missing attribution from present attribution.
- The cross-host limitation is documented alongside the command reference.
