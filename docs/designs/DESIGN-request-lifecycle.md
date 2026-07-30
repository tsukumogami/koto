---
schema: design/v1
status: Proposed
upstream: docs/prds/PRD-request-lifecycle.md
problem: |
  The accepted PRD specifies a request object holding several legs, each
  optionally fulfilled by a child session, with progress appends before
  resolution, per-leg abandonment, and a shell-drivable CLI. koto has no place
  to put such a record: its event log is session-shaped and sessions delete
  themselves on their terminal tick, while the requirement is that a request
  outlive every session it references. Six architectural questions were left
  open at PRD altitude, and they interlock — where the record lives determines
  how the revision is defined, how a delegate finds its own leg, and where a
  terminating child writes its result.
decision: |
  A request is a workspace-scoped directory under `~/.koto/requests/<id>/`
  holding one append-only log with a header line, written through a new
  validate-and-append primitive that takes an exclusive lock for the whole
  check-then-write critical section. Six typed `EventPayload` variants land in
  a new `request.` wire namespace with no schema-version bump. A leg's
  identifiers reach its delegate as additive header fields on the child's own
  state file, surfaced as a `leg` object on the delegate's `koto next`
  response. Abandonment reaches the delegate as a koto-authored notice
  prepended to the authoritative `directive` field. Ten subcommands under
  `koto request` use koto's existing four exit classes, with readiness split
  into a separate polling `wait`. A bound leg's result is promoted on the
  child's terminal tick, between the existing child-log result append and the
  terminal-index write.
rationale: |
  Putting the log at a workspace path rather than in a session resolves the
  central tension without touching a single type: koto's append machinery is
  already path-parameterized, so the event enum is the log format rather than
  the session format. One log per request rather than one per leg keeps the
  revision well-defined as the last event's sequence number and gives a real
  total order across legs, at the cost of a short-held lock on append —
  acceptable because that same lock is what makes at-most-one-result and
  idempotent rebinding enforceable at all, which per-leg files could not
  provide. Every other decision follows koto's own precedent: additive
  serde-optional header fields, the `directive` field the agent-facing skill
  declares authoritative, and the exit-zero-with-discriminator contract
  `koto next` already documents.
---

# DESIGN: request leg-and-result lifecycle

## Status

Proposed

## Upstream Design Reference

Upstream: `docs/prds/PRD-request-lifecycle.md` (Accepted). That document fixes
the requirements (R1–R75) and records the requirement-level decisions. This
design resolves the six questions it deferred and specifies the build.

Two upstream requirements were corrected during this design after
investigation showed them unachievable as originally worded; both corrections
are already in the PRD:

- **R41** originally made the request listings projections of the dispatch
  discovery scan. That scan walks `sessions/` and advances a per-coordinator
  cursor, so it can neither find requests nor satisfy R35's read-only promise.
  R41 now requires vocabulary reuse and forbids a second cursor or claim
  mechanism, without binding the listing to that code path.
- **R73** originally required concurrent writers to different legs not to
  block each other. The enforcement this design needs for at-most-one-result
  and idempotent rebinding is a check-then-write critical section, which does
  briefly serialize. R73 now requires non-corruption with serialization bounded
  to the append.

## Context and Problem Statement

koto records everything in an append-only JSONL event log with a header line,
and the closed `EventPayload` enum is that log's schema. Today every log is a
session log: `LocalBackend` owns the layout under `~/.koto/sessions/<id>/`, and
a session deletes itself on its own terminal tick unless `--no-cleanup` is
passed.

The accepted PRD asks for something that does not fit that shape. A request
holds several legs (R1, R4); several requests can be live in one session (R5);
and critically, a request's record must survive cleanup of the requester's
session *and* of every bound child session, staying readable and listable
afterwards (R6). At the same time the lifecycle must be recorded by typed
variants on the `EventPayload` enum (R53) and the view must be a projection of
those events rather than a separate mutable record (R59).

Read literally those two pull against each other: the event enum is the
session log's schema, and sessions are exactly what a request must outlive.
Resolving that is the first decision, and it constrains the rest — where the
log lives determines what the monotonic revision (R38) can be, how a delegate
discovers its own leg (R21), and where a terminating child writes the result
that resolves its leg (R10).

Four further questions were deferred: the per-leg append bound (R74), the
mechanism by which an abandoned leg's delegate is told (R30), whether the batch
scheduler's container should be extended rather than paralleled (R66), and how
far the CLI contract version travels (R51).

A note on what already exists, because two of these decisions turn on it. The
dispatch protocol's hardened parts — an exclusive-create claim sidecar, a
dispatch-epoch fence, a per-coordinator discovery cursor, an append-only
terminal index, crash-safe respawn — are reused rather than rebuilt (R61). And
koto already has a container-with-named-members primitive in the batch
scheduler, which this design must sit beside without contradicting.

## Decision Drivers

- **Durability outranks locality.** R6 is the hardest constraint in the PRD:
  the record outlives every session it names. A design that stores requests
  inside sessions has to teach every deletion path to make exceptions.
- **The view cannot disagree with the log.** R59 makes the request view a
  projection. Anything cached or denormalized must be reconstructible, and
  where a denormalized pointer exists it must be repairable from events.
- **No new numbers, no new dispatch values.** R47 binds exit statuses to
  koto's documented four classes; R30 forbids a new `action` value. Both exist
  because agents and shell consumers already switch on those closed sets.
- **Additive on the wire, always.** R57 requires older builds to degrade
  through `Unknown` rather than error. That rules out a schema-version bump,
  because the version gate rejects a too-new header before any event is read.
- **Reuse the hardened concurrency primitives.** R61 and R73. koto's
  crash-safety story is the expensive thing here; a second claim mechanism or a
  second cursor would double the surface that has to stay correct.
- **An agent must be able to learn this.** R68 makes the two agent-facing
  skills part of the deliverable. A mechanism a correctly-taught agent would
  not notice is not a mechanism.
- **Bounded reads.** R70 bounds reading one request by its own size. Listing is
  allowed to scale with the number of requests, but reading one is not.

## Considered Options

### Decision 1: Where the request record lives

**Chosen: a workspace-scoped per-request directory with one append-only log.**

```
~/.koto/requests/<request_id>/
  request.jsonl     header line + all events for this request
  request.lock      flock target for the validate-and-append critical section
```

The central tension turns out to be dissolvable rather than tradeable. koto's
append and read primitives in `src/engine/persistence.rs` are
path-parameterized — they take a `&Path` and know nothing about sessions. Only
`LocalBackend` makes a log session-shaped. So `EventPayload` is koto's
*event-log* format, not its *session* format, and a second log family at a
workspace path satisfies R53 literally with no type changes at all.

R6 then becomes a property of the layout rather than a rule enforced at every
deletion site. Sessions delete themselves on their terminal tick, and nothing
in koto walks `~/.koto/` outside `sessions/` and `coordinators/`, so
`koto session cleanup` and `koto workspace prune` need no changes to leave
request records alone.

**Alternative A — events on the requester's session log, with cleanup taught
to preserve them.** Rejected. It inverts the dependency: every current and
future deletion path would need to know about requests, and the requester's
session is not even the right owner, since R5 allows several requests per
session and a request can outlive the session that created it. It also makes
"read one request" a scan of a log containing unrelated workflow events,
against R70.

**Alternative B — one workspace-level append-only log plus a per-request
index, mirroring the terminal index.** Rejected on read cost. The terminal
index is the right precedent for a *scan* — a hot path walked every poll — but
the wrong one for a random read. Reading one request would mean seeking a
shared log that grows with all coordination activity in the workspace, and
compaction would have to preserve request history rather than collapse it.

**Alternative C — one log per leg (`legs/<leg_name>.jsonl`) plus a request
log.** This was seriously considered and is the closest call in the design. It
makes R73's per-leg isolation structural: two legs writing concurrently touch
different files, so no lock is needed at all. It was rejected for two reasons
that compound. First, the monotonic revision of R38 stops being well-defined —
with no shared sequence, a revision has to be synthesized from a count or a
timestamp across files, and two concurrent appends to different legs can
compute the same value. Second, and decisively, the enforcement R12 and R19
require is a *check-then-write*: reject a second result, accept an idempotent
rebind, reject a conflicting one. That is only sound if the check and the write
are atomic with respect to other writers, which needs a lock whether or not
the files are split. Once a lock is on the table, splitting the files buys
ordering weakness rather than concurrency.

**What the per-request choice costs.** `requests/` is the first authoritative
directory outside `sessions/`, so `docs/workspace-layout.md` needs amending.
Request records do not replicate under the cloud backend; that is a documented
limitation, not a silent gap.

### Decision 2: The event family and its wire namespace

**Chosen: six typed variants in a new `request.` namespace, no schema bump.**

| Variant | Wire type | Written by |
|---|---|---|
| `RequestCreated` | `request.created` | `koto request create` |
| `RequestLegBound` | `request.leg_bound` | `koto request bind` |
| `RequestLegProgress` | `request.leg_progress` | `koto request progress` |
| `RequestLegResult` | `request.leg_result` | `koto request resolve`, and the child's terminal tick |
| `RequestLegAbandoned` | `request.leg_abandoned` | `koto request abandon` |
| `RequestClosed` | `request.closed` | `koto request close`, and request-scoped abandon |

Every variant carries `request_id`; the four leg variants also carry
`leg_name`, so a replay can partition by request and route by leg.

`request.` cannot collide with `request_store.` under the existing
`starts_with` check — the two strings diverge at byte 7 (`.` versus `_`), so
neither is a prefix of the other in either direction.

**The namespace is deliberately not reserved in the `fields.kind` space.**
`src/engine/audit.rs` reserves `request_store.` as a prefix for
`EvidenceSubmitted` `fields.kind` values and rejects template authors who use
it. Extending that reservation to `request.` would newly reject a template
author who legally submits `kind: "request.foo"` today, and koto never writes
such a kind — the family is typed variants, not evidence kinds. `koto-author`
gets soft guidance instead.

**Leg disposition is a projection type, not a payload field.**
`LegDisposition { Open, Resolved, Abandoned }` is derived at replay and never
serialized into an event. That makes R59 structural: there is no stored
disposition that can disagree with the events. It is orthogonal to
`TerminalOutcome`, which lives *inside* a resolved leg's result — so a leg that
is `resolved` with a `failure` outcome is normal, not contradictory. It is
deliberately not `TaskOutcome`, whose `pending`, `blocked`, and `spawn_failed`
values have no leg analogue.

**The revision is the sequence number of the last event on the request log.**
Derived, not a new counter. Monotonicity is already enforced by the read path,
which hard-errors on a non-consecutive sequence. Rejected alternatives: a
separate counter (a second thing to keep in step with the log), and an event
count (which can decrease if history is ever compacted, violating R38).

**No `CURRENT_SCHEMA_VERSION` bump.** The header parse rejects a
`schema_version` greater than the current constant as the first step of
reading, so bumping it would make every new log unreadable to an older koto at
line one — converting R57's graceful degradation into a hard error. The
constant has stayed at 1 across several prior additive variants, including
`request_store.result`. The `Unknown` fallthrough was verified directly in the
`Event` deserializer: it reads `type` first and its final match arm is total,
with existing tests covering a dotted type in a reserved namespace.

**Back-compat floor worth recording:** the `Unknown` arm exists from v0.9.0
onward. Builds at v0.8.4 and earlier hard-error on an unrecognized event type,
so R57's promise holds against v0.9.0+, not against all history.

**Alternative — three coarse variants with a nested `LegEventKind`
discriminator.** Rejected. It shrinks the enum but recreates exactly the misread
hazard the PRD rejected one layer down: consumers would match a type string and
then string-match a nested kind, and the feed validator keys on the type string.

### Decision 3: How a delegate learns its own leg

**Chosen: additive header fields on the child's state file, surfaced as a `leg`
object on the delegate's own `koto next` response.**

Two additive `Option<String>` fields — `request_id` and `leg_name` — join the
existing request-store group on `StateFileHeader`, written at bind time after
the `request.leg_bound` event is durable. The delegate reads them as a
top-level `leg` object on its own `koto next` response, mirrored read-only on
`koto status`.

This is the only option that survives post-creation binding, which R18
explicitly allows. An environment variable and the `inputs` payload are both
fixed when the child is spawned, so a leg bound afterwards could never reach a
running delegate. A header rewrite mutates a live session's header, and the
delegate's next tick sees it with no restart. It is also an O(1) reverse
lookup on a read the tick already performs, so the delegate never scans
requests.

**Two constraints this imposes, both load-bearing.**

The `leg` object must **not** carry `dispatch_epoch`. The epoch is baked into
the delegate's dispatch at spawn time, and a freshly-readable epoch would let a
displaced agent present the current value and defeat the fence. Identity is
readable; authority stays baked.

Because a stale agent can still read a valid `leg`, the append path must be
fenced: `koto request progress` and `koto request resolve` take
`--dispatch-epoch` and validate it when the fence applies to the child's
header, exactly as `koto next` does.

`request_id` must be opaque and must not embed the coordinator's session id,
or a delegate-readable field would leak coordinator identity.

**Alternative — a new field on `UnassignedChild` or in the spawn prompt.**
Rejected: both are coordinator-side or spawn-time, so neither satisfies R21's
"readable from its own session" test for a leg bound later.

**Known denormalization.** The header pointer can lag or lose against the
`request.leg_bound` event, mirroring how `assignment_claim` relates to the
claim sidecar. A lost header write leaves a correctly-bound leg whose delegate
reads `leg: null` — degraded capability, not corruption, and repairable from
the event. The bind path treats the event as authoritative and the header write
as best-effort-with-warning.

### Decision 4: How an abandoned leg's delegate is told

**Chosen: a koto-authored stop notice prepended to `directive`, plus an
informational envelope sibling.**

The constraint from R30 is narrow: no new `action` value, and a correctly-taught
agent must notice. That rules out both obvious options.

A new top-level field fails because the agent-facing skill teaches agents to
dispatch on `action` alone and explicitly demotes other top-level fields to
informational. A signal there would be correctly ignored, and D7's cooperative
model would silently degrade into the no-signal alternative the PRD rejected.

A `BlockingCondition` entry fails for two independent reasons. It is
structurally absent from four of the response variants a running delegate can
be sitting in, so it has a coverage hole with no sound fallback. And every
dispatchable field on it already carries a taught meaning that contradicts
"stop": the category values mean "retry later" and "fix something", and the
actionable flag routes an agent to override-or-escalate. Making it work would
need a third category value — breaking a closed enumeration one level below
`action`.

Terminal routing is not merely worse; it is unavailable. koto cannot route to a
state a template did not declare, and templates are not required to declare a
suitable terminal state. Where it is available, reaching terminal fabricates
the child's result, writes a terminal-index entry, emits `ChildCompleted`, and
auto-cleans the session — a cascade in disguise, violating R29.

So the notice goes in `directive`, the one field the skill declares
authoritative and the only one present on every variant a running delegate can
receive. It is applied at the two existing directive-substitution funnels,
after classification and before serialization, so the `action` table and the
gate-derived blocking conditions are untouched. Discovery is one bounded read
of the request record, gated on the child's header carrying a leg binding, and
non-fatal on failure like the existing discovery scan. The advance loop is
deliberately *not* gated on abandonment; gating it would change the delegate's
lifecycle and violate R29.

An informational `leg_abandoned` sibling on the envelope gives a shell consumer
something to branch on without parsing prose.

**Delivery is audited.** A fifth reserved `request_store.`-prefixed
`EvidenceSubmitted` kind is appended once to the delegate's own log when the
notice is first delivered. Without it an operator cannot distinguish "never
told" from "told and ignored". This uses the reserved kind space for exactly
what it was reserved for — an audit record — which is consistent with Decision
2 declining to put *typed control-flow events* there.

**Accepted weakness.** The mechanism is prose in a field, so instruction-
following is the enforcement. The sibling field is what makes it mechanical for
non-agent consumers. The notice is also absent on `koto next --to` responses,
which print without the envelope splice; that path is documented as not
carrying the notice rather than silently differing.

### Decision 5: The CLI surface, the envelope, and the wait

**Chosen: ten subcommands under `koto request`, exit classes 0–3 only, and an
in-process polling wait.**

```
koto request create   [--with-data '{"legs":[…],"inputs":{…}}' | --role R --template T --inputs J]
                      --requested-by ID --coordinator-of-record ID
koto request bind     <request-id> <leg> --child <session-id>
koto request get      <request-id>
koto request wait     <request-id> <predicate> --timeout-secs N [--interval-secs N]
koto request list     [--requested-by ID | --coordinator-of-record ID] [--state open|closed]
                      [--unresolved-legs]
koto request progress <request-id> <leg> --with-data J [--dispatch-epoch N]
koto request resolve  <request-id> <leg> --with-data J [--dispatch-epoch N]
koto request abandon  <request-id> <leg> --rationale TEXT
koto request abandon-request <request-id> --rationale TEXT
koto request close    <request-id>
```

Leg-scoped and request-scoped abandonment are separate subcommands rather than
one subcommand with an optional positional, so an unset shell variable fails
argument parsing instead of abandoning the whole request. `--issued-by` is on
the six mutating verbs and on none of the reads.

**The discriminated state field is `request_state`**, with `open` and `closed`,
alongside `close_disposition` and `leg_counts`. Reusing `action` was rejected:
it is imperative — "what to do next" — and would invite treating `get` as an
advancing poll. Bare `state` was rejected because it already means a template
state name everywhere else in koto. Readiness is deliberately not in the
discriminant, because D8's own argument is that a multi-leg request has no
single readiness answer.

**The contract version is `cli_contract: {"major":1,"minor":0}`** — two
integers rather than a `"1.0"` string, so no consumer can compare it
lexicographically and be wrong. The call-site pin is `--cli-contract
<MAJOR.MINOR>` on every subcommand, validated before any IO, with a mismatch
exiting in the caller-error class. It is modelled on `koto next
--dispatch-epoch`, which is also present-and-compare-before-write. It stays
scoped to this noun group; spreading it to `koto next` and `koto status` is
left as a follow-up rather than done speculatively.

**Exit statuses use only 0, 1, 2, 3.** Zero for every successful read —
including a request with open legs — every successful write, and a satisfied
wait. The transient class for wait timeout, an unsatisfiable predicate, an
interrupted wait, and lock contention. The caller-error class for request or
leg not found (matching where `workflow_not_initialized` already sits), a
malformed identifier, an invalid submission, every rejection under R12, and a
contract-version mismatch. The infrastructure class for persistence failures.

The sysexits values already in use across the crate are 64, 65, 66, and 75, so
none of 0–3 collides. Two traps found and avoided: the existing invalid-session
and invalid-coordinator errors fall through to the transient class, so bubbling
them would break R47's caller-error requirement — this group needs its own
invalid-identifier error at the caller-error class. And because `bind` reuses
the fence, it can legitimately surface 65 and 75, which must not be remapped.

**The wait polls the same read path `get` uses**, with a two-second default
interval, an absolute deadline computed once, and hundred-millisecond sleep
slices so a signal is noticed promptly. Interruption exits in the transient
class rather than zero. Filesystem watching was rejected: it adds a dependency,
is blind under the cloud backend, is defeated by write-temp-then-rename churn,
and it would optimize a single `stat`.

**The predicates** are `--leg <name>` (that leg resolved), `--all-legs`,
`--closed`, and `--resolved-count <N>`; exactly one is required.

### Decision 6: Where a bound leg's result is promoted

**Chosen: an eager append to the request log on the child's terminal tick,
between the existing child-log result append and the terminal-index write.**

The full ordering on a terminal tick becomes:

1. Synthesize the `WorkflowResult` envelope once, hoisted so later steps share it.
2. Append `request_store.result` to the child's own log (existing behavior).
3. **New:** append `request.leg_result` to the request log, under the
   validate-and-append lock.
4. Write the terminal-index entry with its `has_result` flag (existing).
5. Append `ChildCompleted` to the parent's log (existing).
6. Auto-clean the child session (existing).

Step 3 sits before 4 because a crash after the index write would leave a
permanently-skipped session with a forever-open leg. It sits before 5 because
D11 makes the coordinator's own directive the canonical result read, so the
request view must not lag it.

**Two things the existing code forces.** All four existing terminal writes sit
inside an `if !no_cleanup` guard, so a debugging flag would silently disable
result promotion; steps 2–5 must be hoisted out of that guard. And the plain
append assigns its sequence number by reading the last one with no lock, while
the read path hard-errors on a non-consecutive sequence — so concurrent writers
to one request log must go through the lock-guarded path.

**The terminal tick never fails.** Structurally it cannot report one: the
response envelope is printed before these writes and the block ends in a
zero exit. So a promotion failure warns on stderr, and only a retryable IO
error defers cleanup, reusing the existing append-failure lever. A closed
request and an abandoned leg are rejections rather than deferrals, and an
abandoned leg's late result is warn-and-drop, discoverable from the child's own
log where step 2 already put it. Promotion needs no surviving child directory,
because the envelope rides by value.

D10's rejection of an explicit resolve on a bound leg is enforced as a
bound-child check inside the same lock as the write.

**Alternative — lazy projection at read time**, deriving a leg's result from
the child's log when the view is read. Rejected: it fails R59 (the view would
depend on state outside the request's own events), R16 (no recoverable
ordering), R38 (no revision advance on the event that matters most), and R6
(the child's log is exactly what gets cleaned up).

### Decision 7: The batch boundary

**Chosen: keep the containers distinct, and enforce R66 by provenance rather
than reconciliation.**

Independent investigation confirmed the PRD's position and added three reasons
beyond "who supplies the worker". A batch's task list is re-derived from the
latest evidence every tick, so its membership is mutable where a request's legs
are fixed at creation. A batch has no identity of its own — it is addressed by
session-and-state and frozen in a finalization event. And the two counts
already differ correctly today: a batch's total counts submitted tasks
including ones not yet spawned.

**The key finding for R66 is that the dashboard's count is not a batch count at
all.** The rendered task count is derived purely from parent-workflow
parentage, and the dashboard never calls the batch view. So the rule is
provenance, not reconciliation: one count per surface, always over the session
set, with membership as a per-row attribute. Concretely — leave the count logic
untouched, rename the column from `Tasks` to `Children` so it stops implying
batch semantics, add a `membership` attribute (`None`, `Batch`, `Leg`, `Both`)
to the row descriptor derived from the header the tree already holds, render it
as a badge on the member's row, and keep request data out of `koto status`'s
batch section.

The `TaskOutcome`-to-disposition mapping is documented rather than implemented:
`Success` and `Failure` both map to `resolved` (the outcome lives inside the
result), `Skipped` maps to `resolved`, `Pending` and `Blocked` map to `open`,
and `abandoned` has no `TaskOutcome` image at all — which is the clearest
evidence the two enumerations should stay separate.

### Decision 8: The per-leg append bound

**Chosen: 256 appends per leg and 16 KiB per append, rejecting at the bound.**

R74 requires a bound with an explicit behavior. Rejecting is chosen over
truncating and over rolling over: truncation silently loses the newest
information, which is the most valuable, and roll-over breaks R16's ordering
guarantee and R38's monotonic revision. A rejection is a caller error the
consumer can see and react to.

The numbers are set to be far above any plausible legitimate use — a delegate
posting a note per file in a large review stays well inside 256 — and far below
a log-growth hazard: 256 × 16 KiB caps one leg's append contribution at 4 MiB.
Both are operator-tunable under the existing `request_store` config table,
which already holds the dispatch protocol's tunables.

## Decision Outcome

The design hangs together on one insight and one primitive.

The insight is that koto's event log is not its session store. Because the
append and read functions take a path, a request log at a workspace path is the
same format, the same crash-safety machinery, and the same enum — so R53 and R6
stop being in tension and no type changes are needed to reconcile them.

The primitive is a validate-and-append critical section on the request log: an
exclusive lock held across the precondition check and the write. That single
mechanism delivers at-most-one-result, idempotent rebinding with rejection of
conflicting rebinds, exactly-one-winner under concurrency, and a
well-defined monotonic revision — four requirements that otherwise need four
answers. It is also why one log per request beat one log per leg, despite the
latter looking more concurrent.

Everything else follows koto's existing grain. Identifiers reach a delegate the
way the dispatch protocol's other per-child facts do, as additive
serde-optional header fields. The abandonment notice arrives in the one field
the agent-facing skill declares authoritative. The exit statuses and the
exit-zero-with-discriminator envelope match what `koto next` already documents
and what consumers already parse. The result promotion slots into an ordering
that already exists rather than adding a step beside it.

## Solution Architecture

### Components

| Component | Location | Responsibility |
|---|---|---|
| Request store | `src/engine/request_store/mod.rs` | Layout, header type, validate-and-append primitive, read/projection |
| Request view | `src/engine/request_store/view.rs` | Replay events into the view; derive disposition and revision |
| Event variants | `src/engine/types.rs` | The six variants, their wire strings, `LegDeclaration`, `LegResultSource` |
| Leg name validation | `src/engine/batch_validation.rs` | Existing name check, lifted to `pub` and reused |
| CLI noun group | `src/cli/request.rs` | The ten subcommands, envelope serialization, exit mapping |
| Wait loop | `src/cli/request.rs` | Predicate evaluation, deadline, signal handling |
| Promotion hook | `src/cli/mod.rs` terminal path | Step 3 of the terminal ordering |
| Abandonment notice | `src/cli/mod.rs` directive funnels | Notice splice and envelope sibling |
| Header fields | `src/engine/types.rs` | `request_id`, `leg_name` on `StateFileHeader` |
| Dashboard membership | `src/cli/dashboard_state.rs`, `dashboard_render.rs` | `membership` attribute and badge; column rename |

### Key interfaces

```rust
// src/engine/request_store/mod.rs

pub struct RequestHeader {
    pub schema_version: u32,          // 1; independent of the session constant
    pub request_id: String,           // opaque, no embedded coordinator identity
    pub created_at: String,           // RFC 3339 UTC
    pub requested_by: String,
    pub coordinator_of_record: String,
}

pub struct LegDeclaration {
    pub role: String,
    pub template: String,
    pub inputs: serde_json::Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LegDisposition { Open, Resolved, Abandoned }   // projection only

pub struct LegView {
    pub name: String,
    pub declaration: LegDeclaration,
    pub disposition: LegDisposition,
    pub bound_child: Option<String>,
    pub result: Option<WorkflowResult>,
    pub progress: Vec<ProgressEntry>,
}

pub struct RequestView {
    pub header: RequestHeader,
    pub request_state: RequestState,           // Open | Closed
    pub close_disposition: Option<CloseDisposition>,
    pub legs: BTreeMap<String, LegView>,       // canonical order (R75)
    pub revision: u64,                         // last event seq (R38)
}

/// The one write path. Acquires an exclusive lock on `request.lock`,
/// re-reads the log, runs `precondition`, and appends only if it passes.
/// The lock covers check and write together, which is what makes R12,
/// R19 and R73's exactly-one-winner enforceable.
pub fn validate_and_append<F>(
    root: &Path,
    request_id: &str,
    payload: EventPayload,
    precondition: F,
) -> Result<u64, RequestStoreError>
where
    F: FnOnce(&RequestView) -> Result<(), RequestStoreError>;

pub fn read_view(root: &Path, request_id: &str) -> Result<RequestView, RequestStoreError>;

/// Cursor-free walk of `~/.koto/requests/`, parsing only header lines.
pub fn list_requests(root: &Path, filter: &ListFilter) -> Result<Vec<RequestSummary>, RequestStoreError>;
```

### Data flow

Creating a request writes the header and the `request.created` event, then
prints the identifier.

Binding takes the lock, checks the leg exists and is open and either unbound or
bound to the same child, appends `request.leg_bound`, releases, then
best-effort rewrites the child's header with `request_id` and `leg_name`.

A delegate's `koto next` reads its own header; if a leg binding is present it
does one bounded read of the request record, splices any abandonment notice
into `directive`, attaches the `leg` object and any `leg_abandoned` sibling,
and appends the delivery-audit evidence kind once.

A progress append validates the epoch when the fence applies, then takes the
lock, checks the leg is open and within the append bound, and appends.

A child's terminal tick synthesizes its result once, writes it to its own log,
then takes the request lock and appends `request.leg_result` with
`source: promoted` — warning and continuing if the request is closed, the leg
abandoned, or the record unreachable — then proceeds to the index, the parent
event, and cleanup.

Reading projects the log into the view with no writes at all. Waiting calls
that same read on an interval until its predicate holds or the deadline passes.

## Implementation Approach

Phased so each phase is independently testable and the tree stays green.

**Phase 1 — types and the store.** The six variants with their wire strings,
`LegDeclaration`, the disposition and view types, `RequestHeader`, the layout,
`validate_and_append`, `read_view`, `list_requests`. Lift the leg-name check to
`pub`. Unit tests for round-tripping, `Unknown` fallthrough on an unrecognized
`request.` type, canonical ordering, and revision derivation.

**Phase 2 — concurrency and durability.** Lock acquisition and its timeout,
precondition enforcement for at-most-one-result and rebinding, crash-safety
tests for a partial write, and a concurrency test proving exactly one winner
for two simultaneous resolves of one leg.

**Phase 3 — the CLI noun group.** All ten subcommands, the envelope, the
contract version and its pin, the exit mapping, and the wait loop with its
predicates and signal handling. Integration tests per subcommand, including
byte-equality of two reads and exit-status assertions.

**Phase 4 — dispatch integration.** The two header fields, the bind-time header
rewrite, the `leg` object on `koto next` and `koto status`, the epoch fence on
the append paths, and the promotion hook with the terminal ordering and the
hoist out of the cleanup guard.

**Phase 5 — the abandonment notice.** The directive splice at both funnels, the
envelope sibling, the delivery-audit kind, and tests that the `action` table
and gate-derived blocking conditions are unchanged.

**Phase 6 — surfaces and docs.** The dashboard membership attribute and column
rename; `docs/reference/error-codes.md` extended per R69; the two agent-facing
skills updated per R68 and their evals run; `docs/workspace-layout.md` amended
for `requests/`. Two stale statements found during design also get corrected
here: `STABILITY.md` claims the schema constant rises on additive change, which
contradicts both practice and R57, and `DESIGN-session-schema-hygiene.md` still
says the event enum has no catch-all.

## Security Considerations

**Path traversal through identifiers.** Both `request_id` and `leg_name` become
path components. Leg names are validated against the batch task-name grammar,
which admits only alphanumerics, hyphen, and underscore — this is why reusing
that check rather than copying the regex is load-bearing. `request_id` is
koto-generated and opaque, and is validated on every read against the same
character class, so a caller-supplied identifier cannot escape
`~/.koto/requests/`.

**The fence and displaced agents.** Making leg identity readable from a
delegate's own header means a stale or displaced agent can also read it. That
is why the `leg` object deliberately omits `dispatch_epoch` — a readable epoch
would let a displaced agent present the current value — and why the append
paths validate the epoch when the fence applies. Identity is readable;
authority is not.

**Cross-principal writes.** Any process that can read `~/.koto/` can append to
a request log; koto has no principal authentication and this design does not
add one. `--issued-by` is an audit attribution, not an authorization, and is
documented as such so no consumer treats it as an access check. The threat
model is unchanged from the existing session store, where the same property
already holds.

**Identity leakage into delegates.** `request_id` must not embed the
coordinator's session id, because the header field is delegate-readable. An
opaque identifier keeps a delegate from learning its coordinator's identity
through the binding.

**Denial of service through unbounded appends.** Decision 8's bound exists
partly for this: without it a compromised or looping delegate could grow a
request log without limit on the same filesystem koto's crash-safety depends
on. The bound is enforced inside the lock, so it cannot be raced past.

**Lock starvation.** The validate-and-append lock is held only across a
re-read, a check, and one append. A bounded acquisition timeout surfaces
contention as a transient-class error rather than hanging a caller
indefinitely.

**Untrusted content in prose fields.** Progress content, rationales, and
summaries are caller-supplied and are surfaced in a delegate's `directive` and
in the dashboard. They are carried as data and never interpolated into a shell
command or a gate command. The abandonment notice is koto-authored text with
the caller's rationale embedded as a quoted value, so a rationale cannot forge
a second directive instruction.

## Consequences

### Positive

- R6 holds by construction rather than by discipline: no deletion path needs to
  know about requests.
- One lock primitive satisfies four separate requirements, so there is one place
  to get concurrency right and one place to test it.
- No schema-version bump and no changes to existing events, so older builds
  degrade through `Unknown` exactly as R57 promises.
- The revision is the log's own sequence number, so it cannot drift from the
  events it describes.
- Exit statuses and the response envelope match what consumers already parse
  from `koto next`, so one polling idiom covers both surfaces.
- The abandonment notice lands in the field agents are actually taught to obey,
  and its delivery is auditable.

### Negative

- `requests/` is the first authoritative directory outside `sessions/`, so the
  documented workspace layout changes and a new backup or sync concern appears.
- Request records do not replicate under the cloud backend.
- Appends to different legs of one request briefly serialize, which is why R73
  was corrected.
- The delegate's leg pointer is denormalized onto its header and can lag the
  event that is authoritative.
- The abandonment signal is prose in a field for agent consumers; only the
  envelope sibling is mechanical.
- Two containers with overlapping membership remain, and a reader still has to
  learn both.

### Mitigations

- The layout change is documented in `docs/workspace-layout.md` and the cloud
  gap is stated rather than left to discovery.
- The header pointer is repairable from `request.leg_bound`, and the bind path
  warns when the rewrite fails, so the degradation is visible.
- Lock hold time is bounded to one append and contention surfaces as a
  transient error with a retry-safe semantic.
- The envelope sibling gives non-agent consumers a mechanical branch, and the
  delivery-audit kind lets an operator tell "never told" from "told and
  ignored".
- The membership badge and the column rename make dual membership visible on
  the row rather than implied by a count.
