---
schema: design/v1
status: Planned
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
  identifiers reach its delegate through a sidecar file in the child's own
  session directory, surfaced as a `leg` object on the delegate's `koto next`
  response, and the fence reads the bound epoch from the request log so it
  survives the child's cleanup. Abandonment reaches the delegate as a koto-authored notice
  prepended to the authoritative `directive` field. Ten subcommands under
  `koto request` use koto's existing four exit classes, with readiness split
  into a separate polling `wait`. A bound leg's result is promoted on the
  child's terminal tick, between the existing child-log result append and the
  terminal-index write.
rationale: |
  Putting the log at a workspace path rather than in a session resolves the
  central tension cheaply: koto's append primitive is already
  path-parameterized, so the event enum is the log format rather than the
  session format, and only the header-typed read half needs generifying. One log per request rather than one per leg keeps the
  revision well-defined as the last event's sequence number and gives a real
  total order across legs, at the cost of a short-held lock on append —
  acceptable because that same lock is what makes at-most-one-result and
  idempotent rebinding enforceable at all, which per-leg files could not
  provide. Every other decision follows koto's own precedent: the claim
  sidecar's temp-and-rename pointer, the `directive` field the agent-facing
  skill declares authoritative, and the exit-zero-with-discriminator contract
  `koto next` already documents.
---

# DESIGN: request leg-and-result lifecycle

## Status

Planned

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

The central tension turns out to be dissolvable rather than tradeable.
`append_event` in `src/engine/persistence.rs` is genuinely path-parameterized —
it takes a `&Path` and knows nothing about sessions — so `EventPayload` is
koto's *event-log* format rather than its *session* format, and a second log
family at a workspace path satisfies R53 without redefining the enum.

**The read half, however, is not reusable as-is**, and an earlier draft of this
design claimed it was. `parse_header` hard-deserializes `StateFileHeader` and
gates on `CURRENT_SCHEMA_VERSION`; `read_header` and `read_events` both funnel
through it and return `StateFileHeader`; `append_header` is typed to it. Since
`StateFileHeader` requires `workflow` and `template_hash` with no serde
default, a `RequestHeader` line would fail to parse and surface as a corrupted
log.

So two functions get generified rather than copied: a
`read_log<H: DeserializeOwned>(path) -> (H, Vec<Event>)` and an
`append_header_line<H: Serialize>`, with the existing session-typed pair
becoming thin wrappers. Copying instead would duplicate the sequence-gap
validation and the truncated-final-line recovery, which must live in exactly
one place.

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

**Creation is one atomic write, not two.** A header write followed by an event
append would be two fsyncs, and a crash between them would leave a request
whose header parses and whose log is empty — listable, and projecting to a
request with zero legs, because the leg declarations live in the creation
event. That is a direct crash-safety violation, and the pattern that avoids it
already exists in the tree: buffer the header and the first event, fsync a
tempfile in the target directory, then atomically rename with
no-replace semantics. The rename also gives exclusive creation against a
colliding request id for free.

**Three invariants the layout has to defend, because a request log is
multi-writer and immortal where a session log is neither.**

*Every writer must take the lock.* `append_event` derives its sequence number
from an unlocked read of the last one, and the reader hard-errors on any
non-consecutive sequence — which is not the recoverable final-line case. Two
unlocked concurrent appends both computing the same next sequence would brick
the request permanently, breaking R6's promise that the record stays readable.
The structural defense is that the log path stays private to the module: no
public accessor hands a caller a path it could append to directly, and creation
goes through the same discipline.

*A torn tail must be repaired before appending, not after.* An unbuffered
`writeln!` can issue the payload and the newline as separate writes, so a crash
can leave a line with no terminator. The reader recovers that only while it is
the final line; the next append concatenates onto it, making it non-final and
permanently fatal. Session logs mostly dodge this because they have one writer
and are deleted at terminal — a request log has neither property, which is this
design's own durability argument turned against it. Since the lock is already
held, the fix is cheap: after acquiring, verify the file ends in a newline and
the last line parses, and truncate a torn tail before writing.

*Reads must be quiet.* The existing reader prints a warning to stderr on a
truncated final line. A lock-free read racing a concurrent append will hit that
legitimately, and `wait` polls every two seconds, so the request path needs a
quiet variant rather than a stderr stream during normal operation.

**What the per-request choice costs.** `requests/` is the first authoritative
directory outside `sessions/`, so `docs/workspace-layout.md` needs amending.
Request records do not replicate under the cloud backend, and because flock is
host-local the store requires a local filesystem; both are documented
limitations rather than silent gaps.

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

**Three constraints this imposes, all load-bearing.**

The `leg` object must **not** carry `dispatch_epoch`. The epoch is baked into
the delegate's dispatch at spawn time, and a freshly-readable epoch would let a
displaced agent present the current value and defeat the fence. Identity is
readable; authority stays baked.

Because a stale agent can still read a valid `leg`, every mutating leg path is
fenced — `progress`, `resolve`, and leg-scoped `abandon` all take
`--dispatch-epoch`.

**The fence reads the epoch from the request log, not from the child's
header.** This is a correction from the security review and it matters more
than it looks. Reading the child's header would make the fence unavailable in
exactly the window it exists for. The applicability predicate requires
`parent_workflow.is_some() && needs_agent == Some(true)`, which excludes
top-level sessions and batch-spawned children — all of which `bind` would
otherwise happily accept, silently ignoring `--dispatch-epoch` forever after.
Worse, the child's header disappears on its terminal tick while the request
record outlives it, so after cleanup there would be no header to compare
against and the leg would be permanently unfenceable — during precisely the
period a displaced original agent may still be alive.

So `request.leg_bound` carries the child's `dispatch_epoch`, captured at bind
time, and the fence compares against that. Identity stays readable and
authority stays baked; only koto reads the recorded epoch. `bind` also rejects
a child whose header does not satisfy the fence predicate, so a leg cannot be
bound to something unfenceable in the first place.

An unbound leg has no child and therefore no epoch, so it is unfenceable by
construction. That is correct — its creator is the only party that can resolve
it — but it is an exception worth stating rather than leaving implied.

`request_id` is a `ValidatedRequestId` newtype, not a `&str`, so validation
cannot be forgotten at a future call site. koto's existing discipline is
explicit about why: a function taking a validated newtype by reference cannot
be called with an unvalidated string, which is a property the type system
keeps rather than one every author has to re-keep. It is generated in a single
case so two ids cannot collide to one directory on a case-insensitive
filesystem, and it is opaque so requests cannot be enumerated by guessing.

**Alternative — a new field on `UnassignedChild` or in the spawn prompt.**
Rejected: both are coordinator-side or spawn-time, so neither satisfies R21's
"readable from its own session" test for a leg bound later.

**The pointer is a sidecar, not a header rewrite.** An earlier draft of this
design had `bind` rewrite the child's state-file header in place. That is unsafe
here, for a specific reason: the existing atomic header rewrite reads the whole
file and then writes header-plus-tail to a temp and renames, with no lock. Its
current callers all run when the child is not ticking — at claim time and during
stale-sidecar recovery — but R18 explicitly allows binding a leg to a child that
is already running, and a non-batch session holds no lock during `koto next`.
Any event the child appended between that read and the rename would be lost,
including a state transition, which would silently regress the delegate.

So the leg pointer is written as its own temp-and-rename sidecar file in the
child's session directory, following the claim sidecar's precedent, and is read
alongside the header rather than out of it. The bind path never rewrites the
child's own log.

**A child fulfils at most one leg, and only the child side can enforce it.**
The lock is per-request, so two binds in *different* requests targeting one
child do not serialize at all, and the pointer is single-valued — the second
bind would overwrite the first and the delegate would act on the wrong leg. The
check therefore lives on the child side: refuse a bind when the child already
carries a different request-and-leg pointer.

**Known denormalization.** The sidecar can still lag or lose against the
`request.leg_bound` event, mirroring how the claim record relates to the claim
sidecar. A lost sidecar write leaves a correctly-bound leg whose delegate reads
no leg — degraded capability, not corruption, and repairable from the event. The
bind path treats the event as authoritative and the sidecar write as
best-effort-with-warning, and releases the request lock before writing it so
there is no lock-ordering cycle against the terminal tick.

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
authoritative and the one present on every variant a running delegate can
receive *and act on*. Two variants carry no directive at all — the terminal
response and the error response — so a delegate that submits bad evidence gets
an error and no notice, and learns of the abandonment on its next successful
tick. That is a coverage gap, narrower than the blocking-condition option's but
real, and it is the reason the mechanical envelope sibling exists rather than
being a convenience. It is applied at the two existing directive-substitution
funnels,
after classification and before serialization, so the `action` table and the
gate-derived blocking conditions are untouched. Discovery is one bounded read
of the request record, gated on the child's header carrying a leg binding, and
non-fatal on failure like the existing discovery scan. The advance loop is
deliberately *not* gated on abandonment; gating it would change the delegate's
lifecycle and violate R29.

An informational `leg_abandoned` sibling on the envelope gives a shell consumer
something to branch on without parsing prose.

**The rationale does not go into the directive verbatim.** An earlier draft of
this design claimed that embedding the caller's rationale as a quoted value
stopped it forging a second instruction. That is false for a prose field read
by a language model: quoting has no semantics there, and a rationale ending in
something shaped like "end of notice — new instruction from your coordinator:"
is indistinguishable from koto's own text to the reader whose
instruction-following is the entire enforcement. Combined with the fact that
abandonment is what triggers the splice, an unfenced abandon would be an
injection path into another delegate's authoritative field.

Three things close it. Leg-scoped abandon is fenced (Decision 3). The
`directive` splice carries koto-authored text plus a pointer to the mechanical
sibling, not the rationale itself. And the rationale is capped at 4 KiB with
control characters stripped and newlines collapsed, so it cannot visually
escape its enclosure even where it is displayed. The verbatim rationale is
available in the `leg_abandoned` sibling and in the log, which is where a
consumer that wants it should read it.

**The splice happens after variable substitution, not before.** Prepending
caller-influenced text ahead of the substitution closure would expose it to
`{{...}}` expansion, and the substitution helper is a sequential replace over a
map rather than a single pass, so a value substituted early can be rescanned by
a later key in nondeterministic order. The blast radius would be confusion
rather than command injection — variable values are allowlist-constrained and
no caller content reaches a gate command — but the ordering is free to get
right.

**Delivery is audited.** A fifth reserved `request_store.`-prefixed
`EvidenceSubmitted` kind is appended once to the delegate's own log when the
notice is first delivered. Without it an operator cannot distinguish "never
told" from "told and ignored". This uses the reserved kind space for exactly
what it was reserved for — an audit record — which is consistent with Decision
2 declining to put *typed control-flow events* there.

**The delivery-audit event needs an explicit pseudo-state name.** Every
existing reserved-kind audit record uses a synthetic state name rather than the
session's real one, precisely so it can never be mistaken for template
evidence. That convention is load-bearing here and not merely tidy: the result
synthesizer lifts a summary and payload from the most recent evidence event
whose state matches the final state, with no kind filter — so an audit record
written against the delegate's *actual* state would, on a terminal tick, be
promoted as the child's result. The delivery audit therefore writes against a
synthetic `request_store.abandon_notice` state.

**Accepted weakness.** The mechanism is prose in a field, so
instruction-following is the enforcement, and two response variants carry no
directive to splice into. The mechanical sibling is what covers both gaps for a
non-agent consumer. On the directed-transition path the directive funnel does
run, so the notice does appear there; what is missing on that path is the
envelope sibling and the `leg` object, which is the opposite of what an earlier
draft of this design claimed.

**The per-tick read must be cheap.** Checking for abandonment on every tick of
every bound delegate is a read of the request record, which The bounds decision sizes at
up to four megabytes per leg. Reading tens of megabytes to learn one boolean is
not acceptable per-tick cost, so the check short-circuits on the log's
modification time and skips entirely once the delivery-audit marker is present.

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
koto request abandon  <request-id> <leg> --rationale TEXT [--dispatch-epoch N]
koto request abandon-request <request-id> --rationale TEXT
koto request close    <request-id>
```

Leg-scoped `abandon` is fenced alongside `progress` and `resolve`, because
abandonment is what triggers the directive splice and an unfenced abandon would
let any party holding a leg identity reach into a sibling delegate's
authoritative field. Request-scoped `abandon-request` and `close` are
request-level rather than leg-level operations and have no single epoch to
compare against; they record the presented identity for audit instead.

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
wait. The transient class for wait timeout, an interrupted wait, and lock
contention. The caller-error class for request or leg not found (matching where
`workflow_not_initialized` already sits), a malformed identifier, an invalid
submission, every rejection under R12, and a contract-version mismatch. The
infrastructure class for persistence failures.

**An unsatisfiable predicate is a caller error, not a transient one.** An
earlier draft put it in the transient class, which would tell a shell loop to
retry forever on a condition that can never become true — asking for five
resolved legs on a three-leg request, for instance. A structurally impossible
predicate is validated to the caller-error class before polling begins. A
predicate that *became* impossible while waiting, because the legs it needed
were abandoned or the request closed, gets its own caller-error code distinct
from a timeout, so a consumer can tell "not yet" from "never".

The sysexits values already in use across the crate are 64, 65, 66, and 75, so
none of 0–3 collides. Two traps found and avoided: the existing invalid-session
and invalid-coordinator errors fall through to the transient class, so bubbling
them would break R47's caller-error requirement — this group needs its own
invalid-identifier error at the caller-error class. And because `bind` reuses
the fence, it can legitimately surface 65 and 75, which must not be remapped.

**The wait polls the same read path `get` uses**, with a two-second default
interval clamped to a floor so an interval of zero cannot spin, an absolute
deadline computed once, and hundred-millisecond sleep slices so a signal is
noticed promptly. Interruption exits in the transient class rather than zero.
Filesystem watching was rejected: it adds a dependency, is blind under the
cloud backend, is defeated by write-temp-then-rename churn, and it would
optimize a single `stat`.

**Progress and resolve appends carry an idempotency hash.** koto already has a
canonical-JSON idempotency-hash append that short-circuits an identical retry
and raises a conflict when the hash hits but the payload differs. Without it,
"retry on the transient class" is unsafe advice for this group: a `progress`
retried after an ambiguous failure would double-append and burn the append
bound, and a `resolve` retried after a write that succeeded but was not
reported would be rejected as a second result, indistinguishable to the caller
from a genuine double-resolve. Hashing both makes the documented retry
behavior actually safe.

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

**There are two terminal write sites, not one.** The sequence above lives in
the advance-loop completion path, but the identical sequence also exists on the
`koto next --to <terminal>` directed-transition path. Adding promotion to only
one of them would mean a directed transition to a terminal state writes the
child's result, the index entry, and the parent event, then deletes the session
while the leg stays open forever — exactly what ordering step 3 before step 4
exists to prevent. That path also passes its pre-transition event list rather
than re-reading, so a result synthesized there would be computed from a stale
log.

The fix is to extract the whole completion block into one function called from
both sites. That also delivers the synthesize-once hoist, since the result
synthesis is currently invoked separately on each path.

**Only step 3 is hoisted out of the cleanup guard, and it is idempotent.** An
earlier draft hoisted all four writes. That overreaches: an already-terminal
session ticked again returns immediately from the advance loop, so under
`--no-cleanup` the block runs on every tick — hoisting all four would append
another child-log result, another index entry, and another parent event per
tick, unbounded, on a deliberately parked session. It would also change what
`--no-cleanup` means for a large number of existing tests, at least one of which
is built specifically on a parked terminal child *not* emitting the parent
event. So step 3 alone is hoisted, and it is gated on "this leg has no result
yet" so a repeat tick is a silent no-op rather than a warning per tick.

**Every writer takes the lock.** The plain append assigns its sequence number
from an unlocked read of the last one, while the read path hard-errors on a
non-consecutive sequence, so concurrent writers to one request log must go
through the lock-guarded path — which is why the log path stays module-private.

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

**Correction on where membership comes from.** An earlier draft said both
memberships derive from "the header the tree already holds". Leg membership does,
via the new pointer, but batch membership does not — it lives on the
initialization event's spawn entry, and the dashboard's cached session holds the
header plus derived scalars, not events. Batch membership therefore needs a new
cached field populated during the replay that already computes the current
state, rather than being read off the header.

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
to the row descriptor — leg membership from the pointer sidecar, batch
membership from the spawn entry, since the header's `parent_workflow` is set for
any `--parent` child and cannot identify a batch task, render it
as a badge on the member's row, and keep request data out of `koto status`'s
batch section.

The `TaskOutcome`-to-disposition mapping is documented rather than implemented:
`Success` and `Failure` both map to `resolved` (the outcome lives inside the
result), `Skipped` maps to `resolved`, `Pending` and `Blocked` map to `open`,
and `abandoned` has no `TaskOutcome` image at all — which is the clearest
evidence the two enumerations should stay separate.

### Decision 8: The relationship to `koto session start --needs-agent`

**Chosen: the two are orthogonal and both stay supported.** R65 asks which is
the forward path; the answer is that they do different things and neither
replaces the other.

`koto session start --needs-agent` creates a child session that wants an agent.
That is unchanged, still supported, and still the only way a child session comes
into existence — `koto request create` creates the request container and spawns
nothing. `bind` is what connects an existing child to a leg.

So the forward path for a fan-out is create-then-spawn-then-bind, and the
forward path for a single delegation is exactly what it is today, with binding
optional. A request identifier is discoverable from a child created the older
way only once that child has been bound, through the leg-pointer sidecar; a
child that is never bound has no request, which is precisely today's behavior.
This is what makes R60 and R62 hold without a migration: the old path is not
deprecated, it is just no longer the only shape.

### Decision 9: Every bound, not just the append bound

**Chosen: reject at the bound, on five dimensions rather than one.**

R74 names the per-leg append bound, and an earlier draft of this design bounded
only that. The security review's most useful structural finding was that the
unbounded dimensions were the interesting ones — and that koto already ships
enforcement for most of them, so the work is adoption rather than invention.

| Dimension | Bound | Behavior | Source |
|---|---|---|---|
| Progress appends per leg | 256 | reject | new, this design |
| Bytes per progress append | 16 KiB | reject | new, this design |
| Legs per request | 256 | reject at create | mirrors the batch task cap |
| Any JSON flag payload | 1 MiB and depth 128 | reject | reuse the existing inputs and with-data guards |
| `--rationale` | 4 KiB, control characters stripped | reject | tightens the existing 1 MiB rationale precedent |

Rejecting is chosen over truncating and over rolling over: truncation silently
loses the newest information, which is the most valuable, and roll-over breaks
R16's ordering guarantee and R38's monotonic revision. A rejection is a caller
error the consumer can see and react to. All five are enforced inside the lock
where they guard log growth, so none can be raced past.

Two of these are load-bearing beyond hygiene. The **leg cap** matters because
every append re-reads the whole log under the exclusive lock, so an unbounded
leg count makes append cost grow with the record and lengthens the lock hold —
undermining both the bounded-read promise and the claim that the lock is held
only briefly. The **rationale cap** is much tighter than koto's existing 1 MiB
precedent because this rationale is prepended to a delegate's directive on
every tick until it terminates, which makes a large one a context-exhaustion
problem rather than merely a large string.

The append and leg bounds are operator-tunable under the existing
`request_store` config table, which already holds the dispatch protocol's
tunables. The payload and rationale caps are fixed.

**The name grammar is shared, not inherited from the batch validator.** It
moves to a neutral engine module rather than being exposed from
`batch_validation`. Exposing it there would propagate an existing dependency
inversion — that module imports its error vocabulary from the CLI layer, so an
engine-side request store consuming it would end up depending on CLI types —
and would silently make the batch scheduler's reserved action words forbidden as
leg names, which has no justification. The same applies to the temp-and-rename
and root-creation helpers the atomic create needs: they are private to the
session backend today, and the engine must not reach into the session layer, so
they move to a neutral module too.

**Duplicate leg names are rejected at create.** The shared name grammar
validates one name at a time; whole-submission uniqueness is a separate rule in
the batch validator, typed to batch entries and not reusable here. Without an
explicit check, two legs declared with one name would collapse in the view's
map: one declaration silently lost, and a single result resolving what the
caller believes are two legs.

**Retention is a stated gap, not a solved problem.** Nothing in this change
deletes a request, and listing parses every request's header, so the store
grows monotonically and listing gets slower for the life of the workspace. R6
requires durability against *session* cleanup, which does not preclude an
explicit prune, but adding an eleventh verb is scope this change does not take.
It is recorded in Consequences and as a follow-up. Any future prune must not
unlink the lock file while a writer holds it, or two writers would lock
different inodes.

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
| Component-name grammar | `src/engine/name_grammar.rs` (new, neutral) | The shared 1..=64 / `[A-Za-z0-9_-]` / no-leading-hyphen check, consumed by both batch and request validation |
| Atomic create helpers | `src/engine/atomic_fs.rs` (new, neutral) | Temp-and-rename-with-no-replace and root-directory creation, moved out of the session backend |
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

pub struct ProgressEntry {
    pub seq: u64,                              // the event's own seq: ordering (R16)
    pub timestamp: String,
    pub content: BTreeMap<String, serde_json::Value>,
    pub issued_by: Option<String>,
}

pub struct RequestView {
    pub header: RequestHeader,
    pub request_state: RequestState,           // Open | Closed
    pub close_disposition: Option<CloseDisposition>,
    /// The request-level shared context recorded at creation (R3).
    /// Without this the CLI would accept shared inputs and never be
    /// able to read them back.
    pub inputs: Option<serde_json::Value>,
    pub legs: BTreeMap<String, LegView>,       // canonical order (R75)
    pub revision: u64,                         // last event seq (R38)
}

/// One row of a listing. Field names deliberately mirror
/// `UnassignedChild` for the concepts the two share — `requested_by`,
/// `coordinator_of_record`, `created_at` — which is the whole of what
/// R41's vocabulary-reuse clause asks for.
pub struct RequestSummary {
    pub request_id: String,
    pub requested_by: String,
    pub coordinator_of_record: String,
    pub created_at: String,
    pub request_state: RequestState,
    pub leg_counts: LegCounts,
}

pub struct LegCounts {
    pub total: usize,
    pub open: usize,
    pub resolved: usize,
    pub abandoned: usize,
}

pub struct ListFilter {
    pub requested_by: Option<String>,
    pub coordinator_of_record: Option<String>,
    pub state: Option<RequestState>,
    pub unresolved_legs_only: bool,
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

The threat model is a local, single-uid CLI. There is no network service and no
multi-tenancy, so nothing here is a remote-exploit surface. What is real is that
agent-supplied content flows through the store, several processes write
concurrently, and one of those processes can be a *displaced* agent — an
earlier dispatch that is still alive after being superseded. Most of what
follows is about that case and about confused-agent hygiene, not about a
security boundary this tool claims to have.

**Path traversal through the request identifier.** Under Decision 1 only
`request_id` becomes a path component; leg names live inside the log as map
keys, not as directory entries. `request_id` is therefore a
`ValidatedRequestId` newtype whose constructor is the only way to obtain one,
so the read and write entry points cannot be called with an unvalidated
string — validation is a property the type system holds rather than a discipline
each call site re-keeps. It is koto-generated, opaque, and single-case, the last
so two ids cannot collide to one directory on a case-insensitive filesystem,
which would interleave two logs and fail the sequence check permanently.

Leg names are still validated against the shared grammar, but for the wire
format and the CLI rather than for the filesystem, and the grammar additionally
rejects a leading hyphen: leg names are positional shell arguments in every
mutating verb, and a leg named `--rationale` is an argument-parsing hazard for
the shell wrappers this feature exists to serve. koto's own identifier
validator rejects a leading hyphen for the same reason.

**Symlinks and modes.** The request directory and its log refuse to follow
symlinks, matching how the claim sidecar is opened and how workspace prune
refuses a symlinked root. `requests/` and each request directory are created
0700 and the log 0600, rather than relying on the home directory's mode having
been set correctly once.

**The fence and displaced agents.** This is the part the first draft of this
design got wrong, and the correction is in Decision 3: the fence compares
against the epoch recorded in `request.leg_bound`, not against the child's
header. Reading the header would have left the fence unavailable exactly when
it is needed — the applicability predicate excludes children a leg can legally
be bound to, and the header disappears on the child's terminal tick while the
request record outlives it, so a leg would become permanently unfenceable
during the window a displaced agent may still be running.

Every mutating leg path is fenced, including leg-scoped abandon. Abandon is
fenced specifically because it triggers the directive splice, so an unfenced
abandon would be a path for one party holding a leg identity to place prose in
a different delegate's authoritative field. Request-scoped close and abandon
have no single epoch to compare against and are audited with the presented
identity instead. An unbound leg has no epoch and is unfenceable by
construction, which is correct rather than a gap.

**Untrusted content in prose fields.** The abandonment notice reaches a
language model as prose, and quoting has no semantics there — a claim to the
contrary was removed from this design. The rationale is therefore not spliced
into the directive at all: the directive carries koto-authored text plus a
pointer, and the verbatim rationale lives in the mechanical envelope sibling
and in the log. It is capped at 4 KiB with control characters stripped and
newlines collapsed, and the splice happens after variable substitution so
caller-influenced text is never exposed to template expansion. No caller
content reaches a gate command or a shell command on any path.

Terminal escapes are handled by the surfaces rather than here: JSON output
escapes all control bytes, and the interactive dashboard filters any grapheme
containing a control character before it reaches a cell. This change adds only
a derived membership badge to the dashboard and no caller-supplied text, so it
does not widen that surface. If a human-readable renderer for the request view
is ever added, it owns its own scrubbing.

**Denial of service.** The bounds decision covers five dimensions rather than one, all
enforced inside the lock so none can be raced past. The two that matter most
are the leg cap — because append cost and lock hold time grow with the record,
so an unbounded leg count would undermine both the bounded-read promise and the
brief-lock claim — and the tight rationale cap, because that text is prepended
to a delegate's directive on every tick until it terminates, which makes a
large one a context-exhaustion problem rather than a large string. The `wait`
interval is clamped to a floor so it cannot spin.

**Cross-principal reads and writes.** Any process at the same uid can read and
append to a request log. koto has no principal authentication and this design
does not add one; `--issued-by` is audit attribution, not authorization, and is
documented as such so no consumer mistakes it for an access check. This is
unchanged from the existing session store.

One consequence is worth stating plainly rather than implying otherwise: a
delegate holding its own leg identity can read the whole request, including
sibling legs' declarations, progress, and results, and the header's requester
and coordinator. Cross-leg readability is intended — an operator needs it — so
the honest framing is that an opaque identifier prevents enumeration by
guessing and nothing more. It does not hide the coordinator's identity from a
bound delegate. The delegate-facing projections omit the requester and
coordinator fields to avoid handing them over gratuitously, but that is
hygiene, not a boundary.

**Locking.** The lock is an flock, deliberately not an exclusive-create lease
file: the kernel releases an flock when the holding process dies, so a killed
writer cannot strand it, whereas the lease-file pattern elsewhere in koto needs
stale recovery and an age heuristic precisely because those files do strand.
koto has no bounded-wait flock primitive today, so the bounded acquisition is
non-blocking plus sleep-and-retry against a deadline; flock is not
first-in-first-out, so a writer can be starved past its deadline under
sustained contention and surfaces that as the transient class.

Two constraints follow from flock and should be read as limitations rather than
mitigations. The validate-and-append primitive is unix-only, matching koto's
existing platform support. And the store requires a local filesystem: flock is
host-local, so two hosts appending over a network filesystem would collide on
sequence assignment, and the reader hard-errors on a sequence gap — leaving that
request permanently unreadable. That is a worse failure than the session-store
equivalent because this record is the durable one, so it is documented
alongside the cloud-backend gap.

**Retry safety.** Progress and resolve appends carry an idempotency hash so a
retry after an ambiguous failure is a no-op rather than a double-append or a
spurious second-result rejection. Without it, the documented advice to retry on
the transient class would itself be a correctness hazard.

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
- Request records do not replicate under the cloud backend, and the store
  requires a local filesystem because flock is host-local.
- The validate-and-append primitive is unix-only.
- Nothing prunes the request store. It grows monotonically for the life of a
  workspace, and listing parses every request's header, so listing gets slower
  over time. A prune verb is deliberately out of scope for this change and is
  recorded as a follow-up; durability against session cleanup, which is what
  the requirement asks for, does not preclude one.
- A bound delegate can read its whole request, including sibling legs. That is
  intended, but it means an opaque identifier prevents enumeration and nothing
  more — it does not hide the coordinator from a delegate.
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
