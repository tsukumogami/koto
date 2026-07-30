---
schema: prd/v1
status: Accepted
problem: |
  koto's dispatch protocol carries one delegation from creation to a single
  terminal answer. A coordinator running a fan-out — a review panel, a
  bake-off, a parallel research sweep — has no way to see structured progress
  before a delegate finishes, no way to stop waiting on one branch of the
  fan-out, no typed record of a delegation's creation, and no object that
  holds several answers together. Coordination verbs that used to live in an
  external MCP sidecar were removed, so koto is now the only place these
  semantics can live, and no shell-out consumer can drive them today.
goals: |
  Give koto a first-class request object with an ordered, repeatable
  leg-and-result lifecycle layered on the existing dispatch protocol, a CLI
  noun group a coordinator can drive by shell-out with explicit identifiers
  and parseable versioned output, and a typed event family that keeps the
  audit trail complete from creation through close — without changing the
  semantics of the existing dispatch protocol or breaking delegations already
  in flight.
---

# PRD: request leg-and-result lifecycle

## Status

Accepted

koto v0.10.0 shipped a child-session dispatch protocol, and v0.11.x added its
converge half (`docs/prds/PRD-request-store-converge.md`). Together those take
one delegation end to end. This PRD specifies what they do not cover: the
fan-out shape, where one request holds several legs, legs report progress
before they finish, and a coordinator can stop waiting on one of them.

**Vocabulary note.** This document uses **request** for the container and
**leg** for its addressable member. D5 explains why "leg" rather than "slot"
or "task", both of which mean something else already — one in the wider
ecosystem, one inside koto.

## Problem Statement

koto's dispatch protocol takes one delegation end to end. A requester marks a
child session as needing an agent, a coordinator discovers it, wins an
exclusive claim, spawns an agent, and gets woken when the child reaches a
terminal state; the child's result rides back as a typed `WorkflowResult`. For
a single delegation with a single answer this works and is hardened —
exactly-one-winner claims, an epoch fence against stale re-dispatch, an
append-only terminal index, crash-safe respawn.

Coordination in practice is not mostly single delegations. It is panels: a
review panel with five reviewers, a bake-off across three competing
approaches, a research sweep over four subsystems. For that shape, five gaps
are load-bearing.

**A coordinator cannot see progress before a delegate finishes.** The only
mid-flight write a running workflow can make about itself is `koto session
update --intent`, a single free-text field where the last write wins. A
coordinator polling a delegate that has been running for twenty minutes
learns either "still running" or, eventually, the whole answer. Nothing in
between.

**A coordinator cannot stop waiting on one branch of a fan-out.** `koto
cancel` is workflow-scoped: it cancels a workflow. A coordinator holding four
of five verdicts, with a fifth delegate that stalled, has no verb between
"keep waiting forever" and "cancel that delegate's entire workflow," and no
record tying the decision to the delegation that motivated it.

**A delegation's creation is invisible in the log.** A delegation comes into
existence as a side effect of init-time header fields. Nothing typed says
"this was requested, with these inputs, by this principal, for this role."
Replaying a coordinator's log shows dispatch and completion but not the
request's own birth, so the audit trail has a hole at the first question a
reviewer asks: who asked for this, and what did they ask for?

**One delegation carries exactly one answer.** `WorkflowResult` is a single
envelope promoted on the completion tick. A fan-out of N branches is N
separate children with N separate results and no object holding them
together. "What did the panel say?" has to be reconstructed by walking
children and correlating them by naming convention.

**Nothing is queryable at request granularity.** The discovery scan answers
"which children need an agent" and `koto status` answers "what state is this
workflow in." Neither answers "what is the state of the thing I asked for" —
which legs are filled, which are still open, what came back so far.

The coordination verbs that covered some of this — delegate, await, report
progress, update, cancel, finish, query, list — previously lived in an MCP
sidecar in the niwa workspace manager, which removed them
(tsukumogami/niwa#151). koto is now the only place they can live, and nothing
can drive these semantics today: any shell-out consumer — a script, a CI job,
an agent skill — has no surface to call.

koto does already ship one container-with-members primitive — the batch
scheduler's `materialize_children` state, its named task list, and the
`children-complete` gate. It solves a different problem, and D2 states the
boundary rather than leaving a reader to infer it.

## Goals

A coordinator can create one request with several legs, watch answers and
progress land in the order they were produced, stop waiting on a leg that is
not coming, and close the request with a recorded disposition — all from a
shell, from a working directory unrelated to the delegates'.

A reviewer reading the log afterward can reconstruct the whole lifecycle:
creation with its inputs and requester, every progress append, every result,
every abandonment, and the close.

A delegate can report progress without being interrupted and without the
coordinator having to resume it, using only what it can learn from its own
session.

Delegations already in flight when this lands keep working. The existing
dispatch protocol's semantics do not change, and a request with one leg
behaves the way a single `needs_agent` child behaves today.

An agent that already knows koto's surface can use this one without unlearning
anything: the same output shape, the same exit-status vocabulary, the same
rule about which command is safe to poll.

The event family and output shapes are stable enough to parse and to evolve
additively, so a consumer can pin a contract version later without a breaking
change.

## User Stories

### S1 — A coordinator fans out a review panel

As a coordinator agent running a five-reviewer panel, I want to create one
request with five legs, each carrying its own role and its own brief, and read
verdicts as they land, so that I can start synthesizing from the first three
while the last two are still working instead of blocking on the slowest
reviewer.

### S2 — A delegate reports progress without being interrupted

As a delegate agent working through a long review, I want to append a
structured progress note to my leg as I finish each file — using identifiers I
can learn from my own session, without being told them out of band — so that
the coordinator can report what is happening and decide whether to keep
waiting, without resuming me.

### S3 — A coordinator abandons a stalled leg

As a coordinator holding four of five verdicts with a fifth delegate that has
not moved in an hour, I want to abandon that one leg with a rationale and
close the request on four answers, so that the workflow makes progress and the
log records that I chose to proceed short-handed rather than silently dropping
a reviewer.

### S4 — A shell-out consumer drives the lifecycle

As an agent or script driving coordination from a shell, I want every
operation reachable as a single CLI invocation that takes explicit request and
leg identifiers and returns a versioned JSON envelope, so that I work
correctly even though the coordinator and its delegates run in different
working directories, and so that one polling loop works the same way here as
it does against `koto next`.

### S5 — A reviewer audits a closed request

As a human reviewing a completed fan-out weeks later, I want the log to show
the request's creation with its inputs and requester, every progress append in
order, every result, every abandonment with its rationale, and the close with
its disposition, so that I can tell what was asked, what came back, and what
was given up on, without inferring any of it from naming conventions.

### S6 — An existing delegation keeps working

As an operator with delegations in flight when this ships, I want every
existing `needs_agent` child to keep being discovered, claimed, dispatched,
and converged exactly as before, so that upgrading koto does not strand
work-in-progress or require a migration step.

### S7 — A coordinator resumes after a restart

As a coordinator resuming after a restart, I want to list the requests I am
accountable for, see which have unresolved legs, and learn from the request
view alone which delegate session fills each leg, so that I can re-dispatch a
lost leg without having kept my own correlation table.

## Requirements

### Request object and identity

- **R1.** A request is a first-class, addressable object with a lifecycle:
  created, then open while legs are unresolved, then closed.
- **R2.** A request carries an identifier distinct from any child session
  identifier. The identifier is stable for the request's lifetime, is
  generated by koto, and is returned at creation.
- **R3.** A request records, at creation: its legs, any shared context inputs,
  the principal that requested it, and the coordinator accountable for
  servicing it. The principal fields reuse the existing header names
  `requested_by` and `coordinator_of_record`.
- **R4.** A request holds one or more legs. A one-leg request is the degenerate
  case and must be expressible without ceremony — creating one is not more
  work than creating a single `needs_agent` child is today.
- **R5.** More than one request may be live in a single session at the same
  time. A request's identity is not the session's identity.
- **R6.** A request's record is durable independently of every session it
  references. It survives cleanup of the requester's session and of any bound
  child session, and remains readable and listable after both are gone.
- **R7.** In a fan-out the requester and the servicing coordinator are commonly
  the same principal. The two fields stay distinct because the dispatch
  protocol permits a third-party coordinator, so the listings of R39 and R40
  may return the same set.

### Leg declaration and lifecycle

- **R8.** A leg is addressable within its request by a name its creator
  supplies, using the same name grammar the batch scheduler's task names
  already use, so one validation rule covers both.
- **R9.** A leg declaration carries the fields the dispatch protocol requires
  of a child it may be bound to — at minimum `role`, `template`, and `inputs`
  — supplied **per leg**, not per request. A five-reviewer panel gives five
  roles and five briefs in one creation call. Request-level inputs, where
  present, are shared context, not the individual ask.
- **R10.** A leg moves through: open, then resolved with a result, or abandoned
  with a rationale. No other terminal dispositions exist.
- **R11.** Leg disposition is its own enumeration. It is not `TerminalOutcome`
  and not the batch scheduler's `TaskOutcome`, and neither has to change. The
  design states the mapping a reader needs to relate them.
- **R12.** A leg accepts at most one result. A second attempt to record a
  result on a resolved or abandoned leg is rejected with a machine-readable
  reason — not silently dropped, not overwriting.
- **R13.** A leg result reuses the existing `WorkflowResult` envelope shape —
  outcome status, human-readable summary, optional structured payload — so a
  coordinator reads every leg uniformly and a child-fulfilled leg needs no
  translation from the result the child already produces.
- **R14.** A request is closed when every leg is resolved or abandoned, or when
  the whole request is abandoned. Closing records a disposition distinguishing
  "all legs resolved," "closed with abandoned legs," and "request abandoned."
- **R15.** Closing is explicit and recorded. A request with every leg resolved
  is not implicitly closed; the close is the coordinator's statement that it is
  done reading. Closing an already-closed request is rejected under R12's
  rules rather than silently succeeding.
- **R16.** The order in which results, progress appends, and abandonments were
  produced is recoverable from the log, and two readers replaying the same log
  see the same order.

### Binding a leg to a child session

- **R17.** Binding is an explicit operation with a named surface. A leg is
  bound to at most one child session; a child session fulfils at most one leg.
- **R18.** A leg may exist unbound, may be bound after creation, and need not
  ever be bound — an unbound leg can be resolved directly by its creator, or
  abandoned.
- **R19.** Binding is idempotent for the same leg-and-child pair, and is
  rejected under R12's rules when it would rebind a leg already bound to a
  different child.
- **R20.** A bound leg's child session identifier is part of the request view
  of R33. A coordinator holding only a request identifier can learn which
  delegate session fills each leg without keeping external state.
- **R21.** A bound leg's request and leg identifiers are delivered to the
  delegate through the same channel that already carries its dispatch epoch,
  and are readable by the delegate from its own session. A delegate never has
  to be told them out of band, and never has to know the coordinator's
  identifiers to act on its own leg.

### Progress appends

- **R22.** A leg accepts an ordered sequence of progress appends before it
  resolves. Appends are additive: an append never overwrites a previous one,
  and the sequence is readable in production order.
- **R23.** A progress append carries structured content, not only free text. A
  reader can distinguish appends from each other and from the leg's result
  without string-matching prose.
- **R24.** A coordinator can read a leg's appends without resuming, waking, or
  otherwise disturbing the delegate that produced them, and without advancing
  any workflow's state.
- **R25.** `koto session update --intent` keeps its current
  last-write-wins-free-text semantics and is not repurposed as the progress
  mechanism. A workflow's stated goal and a leg's progress are different
  things with different cardinality.

### Abandonment

- **R26.** A single leg can be abandoned with a rationale, leaving the rest of
  the request open.
- **R27.** A whole request can be abandoned with a rationale, abandoning every
  leg still open and closing the request.
- **R28.** The operation is named for what it does. It does not reuse the verb
  `cancel`, which in comparable tools terminates the worker, and which stays
  reserved in koto for the workflow-scoped operation it already names.
- **R29.** Abandoning a leg or request does not cancel the workflow of a
  delegate bound to it. The delegate's workflow is its own audit object with
  its own terminal semantics; a coordinator's bookkeeping does not reach into
  it.
- **R30.** A delegate whose leg was abandoned learns this through koto's
  existing directive-dispatch discipline, without adding a value to the
  `action` enumeration that every agent and both koto skills switch on. A
  correctly-taught agent notices without being given a new field to poll.
- **R31.** `koto cancel`'s existing workflow-scoped behavior is unchanged, and
  no request or leg operation is reachable from it.
- **R32.** Abandonment is recorded with its rationale and the principal that
  issued it. The flag is spelled `--rationale`, matching every other koto
  command that records a reason.

### Read, wait, and list surface

- **R33.** A single read returns a request's whole current view: its legs, each
  leg's declaration, disposition, bound child session, result if resolved, and
  progress appends; plus the request's open-or-closed state with disposition,
  and a monotonic revision.
- **R34.** The read exits zero whenever the fetch succeeded, whatever the
  request's state. Readiness is carried in the payload as a discriminated
  state field, following the shape `koto next` already uses, not in the exit
  status.
- **R35.** Reading a request is read-only. It advances no state, appends no
  event to any log, consumes no cursor, and is safe to call repeatedly.
- **R36.** A separate wait operation owns readiness. It takes its predicate
  explicitly — a named leg resolving, all legs resolving, the request closing,
  or a count of resolved legs — plus a timeout, so "three of five is enough" is
  expressible rather than guessed.
- **R37.** The wait operation exits non-zero only when its predicate was not
  satisfied within the timeout, using the transient class of R47.
- **R38.** The request view carries a monotonic revision that advances on every
  recorded event for that request. A consumer detects change by comparing
  revisions rather than by diffing payloads.
- **R39.** A principal can list the requests it created, filtered at least by
  open-versus-closed.
- **R40.** A coordinator can list the requests it is accountable for servicing,
  filtered at least by "has unresolved legs."
- **R41.** The listings of R39 and R40 reuse the per-entry vocabulary of
  `unassigned_children` on `koto next` responses — the same field names for the
  same concepts — and introduce no second claim mechanism and no second
  discovery cursor. They are read-only per R35, so they do not take the
  cursor-advancing path the dispatch scan uses; a listing never mutates
  coordinator state.

### CLI contract

- **R42.** Every operation lives under a single `koto request` noun group,
  matching koto's convention for every object that is not the workflow itself.
  Request-scoped operations take the request identifier as their first
  positional argument; leg-scoped operations take the leg name as their second,
  matching `koto context add <session> <key>`.
- **R43.** Because the operations live in their own noun group, R31's
  no-accidental-reachability property holds structurally: workflow-scoped
  cancel and request-scoped abandonment are in different argument grammars,
  not in one grammar with a guard.
- **R44.** The operation set covers: create a request with N legs; bind a leg
  to a child session; get one request; wait on a predicate; list requests by
  requester or by servicing coordinator; append progress to a leg; resolve a
  leg with a result; abandon a leg; abandon a request; close a request.
- **R45.** Creating a request with N legs takes its leg list through the
  existing `--with-data` convention — inline JSON or `@file` — the same way the
  batch scheduler takes its task list. The one-leg case stays expressible with
  the flat `--role` / `--template` / `--inputs` flags so R4 holds. No third
  input idiom is introduced.
- **R46.** Every command acting on a request or a leg takes the identifiers as
  explicit arguments and never infers them from the current working directory.
  Coordinators and delegates run in different working directories by
  construction. Where a session backend derives storage location from the
  working directory, that dependence is the backend's and is documented, not a
  property of these commands' identity resolution.
- **R47.** Exit statuses come from koto's documented four-class vocabulary
  (`docs/reference/error-codes.md`) and introduce no new numbers: zero for
  success; the transient class for the wait operation's unsatisfied predicate;
  the caller-error class for a malformed identifier and for a request or leg
  that does not exist, matching where `workflow_not_initialized` already sits;
  the infrastructure class for IO and permission failures. Rejections under
  R12 are caller errors. Nothing collides with the sysexits values koto
  already returns for cap and fence violations.
- **R48.** Machine-readable output is unconditional on stdout with no format
  flag, matching `koto next` and `koto status`. There is no mode in which these
  commands emit prose instead.
- **R49.** Failure output uses koto's structured error envelope — a nested
  object with a code, a message, and details — not the flat legacy shape, and
  the codes are a closed set.
- **R50.** Machine-readable output is a JSON envelope with stable field names
  that evolves additively: new fields may appear, existing fields do not change
  meaning or disappear. Consumers ignore fields they do not recognize.
- **R51.** The envelope carries a two-part CLI-contract version. The minor part
  advances on additive change; the major part is the only breaking move. The
  field name is unambiguous and contains no form of the word `schema`, because
  `schema_version` already names the on-disk state and log format and is a
  different thing. A caller can also pin the contract version at the call site
  rather than only detecting it in the response.
- **R52.** Two reads of an unchanged request return byte-equal envelopes. The
  view carries no read-time clock; R38's revision is how a consumer tells fresh
  from stale.

### Event family and audit

- **R53.** The lifecycle is recorded by a typed event family on koto's closed
  `EventPayload` enum, with one variant each for: request creation, leg
  binding, leg progress append, leg result, leg abandonment, and request close.
- **R54.** Abandonment has its own variant. It cannot ride the result variant,
  because `WorkflowResult`'s status enumeration has no abandoned value and R12
  forbids a result on an abandoned leg; and it cannot ride the close variant,
  because R26 leaves the rest of the request open.
- **R55.** The family's wire type strings live in a namespace of their own,
  distinct from `request_store.`. That prefix is already reserved for
  `EvidenceSubmitted` `fields.kind` values — template authors are rejected for
  using it — so placing typed event types there would overload one prefix
  across two layers of the log format.
- **R56.** `request_store.result` keeps its current wire string and meaning:
  the child workflow's own terminal result. It is not renamed, not moved into
  the new namespace, and not redefined as a leg-level event.
- **R57.** A koto build predating these events reads a log containing them
  without failing: unrecognized type strings fall through the existing
  `Unknown` variant, so older builds degrade to not understanding the new
  events rather than to a parse error.
- **R58.** The existing events keep their current meaning. `ChildCompleted`,
  `RequestStoreResult`, and the reserved `EvidenceSubmitted` audit kinds
  (`ChildDispatched`, `ChildRedelegated`, `RequesterWoken`,
  `RequesterRespawn`) are not redefined, renamed, or given new fields whose
  absence changes how an existing consumer reads them.
- **R59.** Replaying a request's recorded events reconstructs the R33 view
  exactly, wherever those events are written. The view is a projection of the
  log, not a separate mutable record that can disagree with it. Creation, every
  binding, every append, every result, every abandonment, and the close are
  each individually recorded; no lifecycle step is inferable-only.

### Compatibility

- **R60.** An existing `needs_agent` child created before this feature lands
  continues to be discovered, claimed, dispatched, woken, and converged with no
  change in behavior and no migration step.
- **R61.** The hardened mechanisms of the dispatch protocol are reused, not
  duplicated: the exactly-one-winner claim, the dispatch-epoch fence, the
  discovery cursor, the terminal index, and the respawn path serve leg binding
  as they serve child dispatch today. This feature introduces no second claim
  mechanism and no second discovery scan.
- **R62.** A one-leg request whose leg is bound to a child produces the same
  observable dispatch and converge behavior as today's single `needs_agent`
  child, plus the new creation, binding, append, and close records.
- **R63.** For a bound leg, the coordinator's own `koto next` directive remains
  the canonical place to read the child's result, as the koto-user skill
  already instructs. The request view is the surface for progress, partial
  state, cross-session reads, and restart recovery. Stating this precedence
  keeps the two from becoming competing answers to one question.
- **R64.** `koto next` remains the only advancing poll. R35's read-only promise
  covers the request read and `koto status`, and explicitly does not extend to
  `koto next`, which appends events and advances the discovery cursor.
- **R65.** The relationship between creating a request and `koto session start
  --needs-agent` is stated: which is the forward path for a new delegation,
  whether the older form remains supported, and how a request identifier is
  discoverable from a child created the older way.
- **R66.** The request container does not duplicate the batch scheduler's
  container semantics. Where a request's legs and a batch's tasks describe
  overlapping structure, the design either extends the batch container or
  states why it cannot; two containers whose member counts can disagree about
  the same child sessions are forbidden.
- **R67.** The request identifier of R2 and the existing `child_session_id`
  join key coexist. Consumers joining on `child_session_id` today keep working;
  the request identifier is the additional key making the many-leg shape
  addressable, and `child_session_id` now identifies a leg's fulfiller rather
  than a request.

### Teaching surface

- **R68.** The `koto-user` and `koto-author` skills are updated in the same
  change that lands the surface, covering at least: the request noun group,
  the precedence rule of R63, the progress-versus-intent distinction of R25,
  the abandonment signal of R30, and the exit-status table of R47.
- **R69.** `docs/reference/error-codes.md` gains a section for these commands,
  and any statement in either skill that this feature makes false is corrected
  in the same change.

### Non-functional

- **R70.** Reading a request is bounded work: cost grows with that request's own
  legs and appends, not with the number of requests in the workspace or the
  length of any unrelated log.
- **R71.** The discovery scan's per-poll cost does not grow with the number of
  progress appends. Appends are not on the scan's hot path.
- **R72.** Every recorded step survives a crash at any point: a step is either
  durably recorded or not recorded, never half-recorded, and a reader after a
  crash sees a consistent view.
- **R73.** Concurrent writers to different legs of the same request never
  corrupt each other or each other's records, and any serialization between
  them is bounded to the append critical section rather than held across a
  leg's lifetime. Concurrent attempts to resolve or bind the same leg produce
  exactly one winner.
- **R74.** Progress appends are bounded in size and count per leg, with the
  bound recorded and an explicit behavior when it is hit. An unbounded append
  stream is a log-growth hazard on the substrate koto's crash-safety depends
  on.
- **R75.** The request view has a canonical serialization order for legs, so
  R52's byte-equality is well-defined for a named collection.

## Acceptance Criteria

### Request object and legs

- [ ] A request can be created with N legs in one CLI invocation and is
      immediately readable with all N legs open.
- [ ] The request identifier returned at creation is accepted by every
      request-scoped operation and is not equal to any child session
      identifier.
- [ ] A five-leg request can be created with five distinct roles and five
      distinct input payloads in one invocation.
- [ ] A one-leg request can be created with the flat role/template/inputs flags
      and no more arguments than a single `needs_agent` child requires today.
- [ ] Two requests can be live in one session simultaneously, and operations on
      one leave the other unchanged.
- [ ] Creating a request records the per-leg declarations, the requesting
      principal, and the servicing coordinator, all readable from the log.
- [ ] A request remains readable and listable after the requester's session and
      every bound child session have been cleaned up.
- [ ] Two legs in one request cannot share a name, and a name outside the batch
      task-name grammar is rejected.

### Leg lifecycle

- [ ] Resolving an open leg records its result; the result is readable through
      the request view.
- [ ] A second result on a resolved leg is rejected with a machine-readable
      reason and leaves the first result intact.
- [ ] A result recorded on an abandoned leg is rejected the same way.
- [ ] A leg result read back through the request view has the same status,
      summary, and optional payload fields as the `WorkflowResult` the child
      produced.
- [ ] A request with all legs resolved reports open until closed explicitly.
- [ ] Closing records a disposition distinguishing all-resolved,
      closed-with-abandoned-legs, and request-abandoned.
- [ ] Closing an already-closed request is rejected, not silently accepted.

### Binding

- [ ] A leg can be bound to a child session, and the binding is visible in the
      request view.
- [ ] Rebinding the same leg to the same child succeeds without changing
      anything; rebinding to a different child is rejected.
- [ ] An unbound leg can be resolved directly by its creator.
- [ ] Resolving a bound leg explicitly is rejected with a distinct
      machine-readable reason.
- [ ] A child bound to a leg that completes normally resolves its leg with no
      additional coordinator or delegate action, and does not fail its own
      terminal tick.
- [ ] A delegate can append progress to its leg using only information
      available from its own session, with no identifier passed in its prompt.
- [ ] Given only a request identifier after a restart, a coordinator can
      determine which child session fills each leg from the request view alone.

### Progress appends

- [ ] Ten appends to one leg are all readable, in the order they were made.
- [ ] An append never changes or removes a previous append.
- [ ] An append is distinguishable from a leg result by field structure, not by
      parsing text.
- [ ] Reading a leg's appends appends nothing to any log, advances no workflow
      state, and does not advance the discovery cursor, verified by comparing
      log length, workflow state, and cursor before and after.
- [ ] `koto session update --intent` still overwrites the workflow's intent and
      does not create a progress append.
- [ ] Exceeding the per-leg append bound produces the documented behavior, not
      silent truncation or unbounded growth.

### Abandonment

- [ ] Abandoning one leg of a three-leg request leaves the other two open and
      the request open.
- [ ] Abandoning a request abandons every open leg and closes the request.
- [ ] After abandoning a leg bound to a running child, that child's workflow is
      still in its pre-abandonment state and is not cancelled.
- [ ] A delegate whose leg was abandoned is routed to stop by the same `action`
      dispatch it already follows, without a new `action` value and without
      polling a new field.
- [ ] `koto cancel <workflow>` behaves exactly as it does today and does not
      abandon any request or leg.
- [ ] No request or leg operation is reachable from `koto cancel`.
- [ ] Every abandonment is readable from the log with its rationale and issuing
      principal, recorded through a `--rationale` flag.

### Read, wait, and list

- [ ] One read returns leg declarations, dispositions, bound child sessions,
      results, appends, the request's open-or-closed state with disposition, and
      a revision.
- [ ] The read exits zero for a request with open legs, for a fully resolved
      request, and for a closed request alike.
- [ ] The read's payload carries a discriminated state field a consumer branches
      on without parsing prose.
- [ ] Waiting on a named leg returns zero once that leg resolves.
- [ ] Waiting with a count predicate returns zero once that many legs have
      resolved, without requiring all of them.
- [ ] A wait whose predicate is unsatisfied at timeout exits in the transient
      class.
- [ ] The revision advances on every recorded event for the request and never
      decreases.
- [ ] Listing by requester returns requests that principal created, filterable
      to open only.
- [ ] Listing by servicing coordinator returns requests with unresolved legs.
- [ ] The listings' per-entry field names match those of `unassigned_children`
      on `koto next` responses.

### CLI contract

- [ ] Every operation is a subcommand of one `koto request` noun group.
- [ ] Request-scoped operations take the request identifier as their first
      positional argument; leg-scoped operations take the leg name second.
- [ ] Every operation runs as a single CLI invocation with no daemon running.
- [ ] Every operation succeeds when invoked from a working directory unrelated
      to any workflow's directory, given explicit identifiers, on the default
      session backend.
- [ ] No operation resolves a request or leg identifier from the current
      working directory.
- [ ] Machine-readable output is emitted with no format flag passed, matching
      `koto next`.
- [ ] Failure output is the structured nested error envelope with a code from a
      closed set, not the flat legacy shape.
- [ ] Exit statuses fall within koto's documented four classes and collide with
      none of the sysexits values koto already returns.
- [ ] A request or leg that does not exist exits in the caller-error class,
      matching `workflow_not_initialized`.
- [ ] The envelope carries a two-part contract version whose field name contains
      no form of the word `schema`.
- [ ] A caller can pin the contract version at the call site.
- [ ] Adding a field to the envelope advances the minor version and does not
      break a consumer that ignores unknown fields.
- [ ] Two consecutive reads of an unchanged request produce byte-equal
      envelopes, with no field excluded.
- [ ] Legs serialize in a canonical order stable across reads.

### Events and audit

- [ ] Creation, each binding, each append, each result, each abandonment, and
      the close each produce their own log entry.
- [ ] Abandonment is recorded by its own event variant, not by a result or a
      close.
- [ ] No new wire type string begins with `request_store.`.
- [ ] The existing `request_store.` `fields.kind` reservation still rejects
      template-author use, unaffected by this feature.
- [ ] `request_store.result` retains its wire string and its meaning as the
      child workflow's terminal result.
- [ ] A koto build without these variants reads a log containing them, surfaces
      them through the `Unknown` fallthrough, and does not error.
- [ ] Replaying a request's events reproduces the request view returned by a
      live read, field for field.
- [ ] Logs written before this feature replay to the same view they do today.

### Compatibility

- [ ] A `needs_agent` child created by the previous koto version is discovered,
      claimed, dispatched, woken, and converged after upgrade, with no
      migration step.
- [ ] The claim path used for leg binding is the existing claim sidecar; no
      second claim file format or second discovery scan exists.
- [ ] Two coordinators racing to bind the same leg produce exactly one winner.
- [ ] A one-leg request bound to a child produces the same dispatch and converge
      observables as today's single-child path.
- [ ] A consumer joining on `child_session_id` continues to resolve the same
      child from the same key.
- [ ] A coordinator following the koto-user skill's canonical-source rule reads
      a bound leg's result from its own directive and still sees that result in
      the request view.
- [ ] The batch scheduler's task counts and a request's leg counts never
      disagree about the same set of child sessions in any rendered surface.
- [ ] A delegation created via `koto session start --needs-agent` behaves as the
      documented relationship in R65 states.

### Teaching surface and non-functional

- [ ] Both koto skills are updated in the same change, and no statement in
      either is left false by this feature.
- [ ] `docs/reference/error-codes.md` documents these commands' statuses.
- [ ] Reading one request's view does not read any other request's legs or
      appends.
- [ ] Discovery-scan cost per poll is unchanged by the number of progress
      appends in the workspace, measured across a workspace with none and one
      with many.
- [ ] A process killed mid-write leaves either the complete step or no step, and
      the next read succeeds.
- [ ] Concurrent appends to different legs of one request all land.

## Out of Scope

- **Peer messaging and mention routing.** Fire-and-forget signals between
  agents, an inbox, and mention resolution are a separate capability with a
  separate event family. This PRD covers the task-lifecycle shape only. A
  synchronous question-and-reply is in scope here, because it is a one-leg
  request with a result rather than a fire-and-forget signal.
- **Changes to the dispatch protocol's semantics.** Discovery, claiming, the
  epoch fence, the wake pass, respawn, and the terminal index keep their
  current behavior. This feature adds a layer above them; it does not
  renegotiate them.
- **Changes to the batch scheduler's semantics.** R66 forbids a second
  container that can disagree with the batch, and D2 states the boundary, but
  redesigning `materialize_children`, the `children-complete` gate, or
  `TaskOutcome` is not this feature's work.
- **Forced termination of a delegate.** koto has no process control. R30 gives a
  stop signal a delegate can act on; making a delegate stop is the agent
  runtime's job, not koto's. A future compound operation that both abandons a leg and cancels the
  bound workflow is a convenience, not a primitive.
- **Dashboard panel design.** R66 constrains the request view and the batch's
  counters not to contradict each other, and D2 fixes the visual relationship.
  The panel's layout, interaction, and visual treatment are the dashboard's own
  work.
- **A consumer-side version-pin discipline.** R51 requires a two-part version
  and a call-site pin. What a consumer does with it — which versions are
  compatible, what the verification step is, what happens on mismatch — is the
  consumer's design, not koto's.
- **Implementation.** No architecture, no task breakdown, no code.

## Capability Coverage

The coordination verbs that motivated this feature divide into three groups.
Naming what is already covered keeps the scope honest.

| Capability | Status | Surface |
|---|---|---|
| Delegate work to an agent | Already covered | `needs_agent` plus discovery and claim. Pull-based rather than push. The multi-leg form is the fan-out generalization; R65 states which command is the forward path. |
| Await a delegation | Covered in part; completed here | The wake pass and `RequesterWoken` cover the single-delegation case. R36's wait operation adds a request-scoped predicate with an explicit timeout. |
| Report a final answer | Already covered | `RequestStoreResult` carrying `WorkflowResult`, promoted on the completion tick. Newly recorded against a leg (R13, and the promotion rule in D10). |
| Query one delegation | Partial — completed here | The discovery scan and `koto status` answer workflow-shaped questions. R33 adds the request-scoped read. |
| List outstanding delegations | Partial — completed here | R39 and R40, as projections of the existing scan (R41). |
| Post a mid-flight update | Missing — this PRD | Leg progress appends (R22–R24). |
| Report incremental progress | Missing — this PRD | The same mechanism as a mid-flight update; no dedicated progress event (D6). |
| Stop waiting on one delegation | Missing — this PRD | Leg-scoped and request-scoped abandonment (R26–R32). |
| Ask a question and get one answer | Missing — this PRD | A one-leg request with a result. |
| Send a fire-and-forget signal | Out of scope | Peer messaging; see Out of Scope. |

## Deferred to Design

The design resolves these. Each is an implementation-architecture choice whose
requirement-level constraint is already fixed above.

1. **Where the request record lives.** R6 pins that it outlives the requester's
   session and every bound child; R59 pins that the view is a projection of the
   recorded events. Whether those events live on the requester's session, the
   coordinator's, or a workspace-scoped store is open, and the answer changes
   what a coordinator restart reconstructs and what `koto session cleanup` and
   `koto workspace prune` must preserve.
2. **The append bound.** R74 requires bounded size and count per leg with a
   documented behavior at the bound. The numbers, and whether the behavior is
   reject / truncate-with-marker / roll-over, are design scope.
3. **How a delegate learns its leg was abandoned.** R30 fixes the constraint —
   through the existing `action` dispatch, without a new `action` value.
   Plausibly a blocking condition with an actionable directive, or a route to a
   terminal state. Which mechanism, and what the delegate sees, is open.
4. **Whether the batch container is extended.** R66 forbids contradictory
   counts and D2 states the spawn-path difference that makes the two distinct
   today. Whether the design unifies them anyway, and at what cost to the
   batch's in-process spawn path, is a design call.
5. **Whether the contract version spreads.** These would be koto's first
   versioned CLI envelopes. Whether `koto next` and `koto status` grow the same
   field, or the version stays scoped to this noun group, is open; a pin
   covering one command family but not the one beside it is a partial
   guarantee.
6. **The request-identity-to-session-identity mapping.** D1 reverses an earlier
   simplification in which the request and the child session were one object.
   The concrete mapping, and whether any existing field's documented meaning
   needs amending, is design scope.

## Known Limitations

- **Assignment stays pull-based.** A coordinator finds work by scanning, so the
  latency between a request being created and a leg being bound is bounded by
  the poll interval, not by an event delivery. That is inherited from the
  dispatch protocol and is not renegotiated here.
- **Abandonment cannot stop a delegate.** D7's cooperative signal is the most
  koto can promise without process control. A delegate that never checks in
  again keeps working on an abandoned leg until it finishes.
- **Reintroducing request identity has a cost.** D1 reverses the earlier
  collapse of request-into-child, and four existing names change what they
  denote. Doc comments in the code still describe the old meaning, so a reader
  who greps before reading D1's table will be misled until those are updated.
- **Two containers, one member type.** D2 keeps the batch container and the
  request container distinct, and both can have child sessions as members. R66
  prevents contradictory counts in rendered surfaces, but the conceptual cost
  of two fan-out models is real and is bounded rather than eliminated.
- **"Request" remains overloaded.** D4 and D5 fix the wire namespace and the
  member noun, but the config table, the header field group, an operator-facing
  error string, and the koto-user skill all use "request" for the `needs_agent`
  flag. Full disambiguation would need a deprecating rename on the existing
  side, which this PRD does not propose.
- **Several requests per session raises a growth question.** The dispatch
  protocol assumed one request per session; allowing several, each with an
  append sequence, puts more in one session's log and possibly its state file.
  R70, R71, and R74 fix the properties; the design picks bounds that hold, and
  the numbers are not set here.
- **Partial results are the coordinator's judgment call.** R36 lets a
  coordinator wait for a count rather than for everything, and closing on four
  of five is supported and recorded, but nothing tells a coordinator whether
  four is enough. That is a workflow-authoring decision, and this substrate
  deliberately has no opinion.

## Decisions and Trade-offs

### D1 — The leg-and-result lifecycle layers on the dispatch protocol

**Decision:** Layer on. The new event family is built above the v0.10.0
dispatch protocol, reusing its claim, discovery, epoch-fence, terminal-index,
and respawn machinery, rather than standing beside it as a parallel model.

**Alternatives:** (a) layer the new family on the existing substrate; (b) build
a parallel request store with its own storage, claiming, discovery, and
convergence, leaving the dispatch protocol as a separate primitive.

**Why layer on.** Both models want the same per-session state file and the same
header, so standing beside would mean a second discovery scan, a second claim
mechanism, a second wake path, and a second terminal index — and those four are
exactly the hardened parts. The claim sidecar's exclusive create, its fsync
ordering, the epoch fence against stale re-dispatch, and the mtime cursor were
the expensive things to get right; rebuilding them for a second model buys
nothing and doubles the crash-safety surface that has to stay correct. The
converge work also already established that a typed variant can join the closed
event enum at no cost to older readers, because unrecognized type strings fall
through `Unknown`. And layering keeps delegations already in flight working
unmodified — the new events are additive, and a request with no leg events
behaves like today's single-child request — while standing beside doubles every
coordinator integration's code paths, so every integration would need two code
paths and a per-call decision about which model applies.

**The counter-argument, stated fairly.** The granularity mismatch is real. The
dispatch protocol models one child workflow per request and is terminal-only,
and that was deliberate: the request and the unassigned child are the same
object, which is why the join key is a child session id and why the header
carries `requested_by` and `coordinator_of_record` directly. The leg model
wants the opposite shape — one request holding N legs, several requests live in
one session, and appends before anything is terminal. Layering a many-leg
container onto a substrate whose identity *is* a child session id is not free.
It reverses a settled simplification.

**How the decision resolves it.** Layer on, and deliberately reintroduce the
container the collapse removed (R1, R2, R5): a request identity distinct from
any session id, with each leg bound to at most one child session (R17). The
child session stops being the request and becomes the fulfiller of a leg. The
existing one-child case is then the degenerate one-leg request, expressible with
no extra ceremony (R4, R62), and the `child_session_id` join key keeps working
alongside the new identifier (R67). This is the smallest change admitting the
many-leg shape while keeping the hardened machinery. Standing beside would be
right if the two models had different durability or concurrency needs — they do
not. A leg binding is the same exactly-one-winner problem the claim sidecar
already solves (R61, R73).

**What a reader of the current code must update.** The reversal changes what
four existing names denote, and doc comments in the tree still say the old
thing:

| Name | Meant | Now means |
|---|---|---|
| `child_session_id` | the request | a leg's fulfiller |
| `requested_by` (on a header) | who asked for this request | who asked for the request this leg belongs to |
| `coordinator_of_record` (on a header) | who owns this request | who services the request this leg belongs to |
| `needs_agent` | this session is a request | this session is bound to a leg awaiting dispatch |
| `request_store.result` | the request's result | the child workflow's own terminal result (R56) |

### D2 — A request is not the batch scheduler's container, and the difference is who supplies the worker

**Decision:** The request container and the batch scheduler's
`materialize_children` container stay distinct primitives. R66 forbids them
disagreeing about the same child sessions, and the design either extends the
batch container or states why it cannot.

**Alternatives:** (a) distinct containers with a stated boundary; (b) a request
*is* the batch generalized, and legs *are* batch tasks; (c) say nothing and let
readers guess.

**Why distinct.** koto already ships a container with named members that
converges — a `materialize_children` state with a validated task list, the
`children-complete` gate, and a frozen final view. Superficially that is this
feature's shape, and not saying so is what would make the new member noun read
as a synonym for something koto already has.

The two differ on the axis that decides everything else: **who supplies the
worker.** A batch-spawned child is created by the scheduler itself, in the
scheduler's own process, and is written with `needs_agent: None`
(`src/cli/init_child.rs`). koto's epoch module says so explicitly — the
dispatch fence applies only to children with `needs_agent == Some(true)`, and
batch children are excluded because the dispatched agent is the same process as
the spawning batch scheduler, so there is no claim, no dispatch, and no fence
(`src/engine/epoch.rs`). A request's leg is the opposite case by construction:
it needs an agent assigned by a *different* process, which is the whole reason
the claim sidecar and the epoch fence exist.

That difference is why (b) fails. Declaring legs to be batch tasks would either
drag agent assignment, mid-flight progress, and per-member abandonment into a
container deliberately built without them, or quietly give one word two
meanings — a scheduler-spawned task and an agent-dispatched leg. It is also why
the member is not called `task` (D5).

**What this costs, and the constraint it buys.** Two containers is more concept
than one, and their members can both be child sessions, so a reader can see a
batch task count and a request leg count describing overlapping sets. R66 makes
that a requirement rather than an accident: no rendered surface may show
counters that disagree. For the dashboard specifically, the request view
annotates the existing session tree rather than introducing a second tree — a
child session appears once, with its leg membership as an attribute — so a
human never has to reconcile two hierarchies. Whether the cleaner long-run
answer is one unified container is a real question and belongs to a design that
can weigh the batch's spawn path.

### D3 — New typed `EventPayload` variants rather than reserved evidence kinds

**Decision:** The family lands as typed `EventPayload` variants (R53), not as
`EvidenceSubmitted` entries with reserved `fields.kind` values.

**Alternatives:** (a) typed variants; (b) reuse `EvidenceSubmitted` with
reserved `kind` names, the pattern the four existing dispatch audit events use;
(c) one generic leg event with a stringly-typed discriminator.

**Why.** The reserved-kind pattern is right for what it covers.
`ChildDispatched`, `ChildRedelegated`, `RequesterWoken`, and
`RequesterRespawn` (`src/engine/audit.rs`) are audit records that nothing
matches on exhaustively and nothing type-checks. A reserved key inside a
free-form fields map is fine for that.

This family is different in kind. Leg progress and leg results are read on the
coordinator's poll path and drive control flow — whether to keep waiting,
whether to synthesize, whether to close. A discriminator string inside
`EvidenceSubmitted.fields` gives no exhaustiveness, so a missing case is a
silent misread instead of a compile error; it collides with template authors'
own evidence keys in the unprefixed namespace; and it cannot carry a typed
`WorkflowResult` without hand-rolling the parse on every read. koto's own idiom
already rejects that trade — `TerminalOutcome` is a typed enum specifically so
consumers match exhaustively and typos are not silent miscategorizations.

Adding variants is also cheap in a way it was not before `RequestStoreResult`
shipped: an unrecognized type string falls through `Unknown`, so a new variant
costs an older reader nothing (R57). The reserved kinds stay reserved and
untouched (R58).

### D4 — The new events get their own wire namespace, not `request_store.`

**Decision:** The family's wire type strings live in a namespace distinct from
`request_store.` (R55). `request_store.result` stays exactly as it is (R56).

**Alternatives:** (a) a new namespace of its own; (b) extend `request_store.*`,
on the grounds that the converge work already established it as a reserved
event-type prefix.

**Why (b) is wrong.** `request_store.` is not only an event-type prefix. koto
reserves it as a prefix for `EvidenceSubmitted` `fields.kind` values, and
template authors are *rejected* at submission time for using it — the
reservation exists to give koto headroom for future audit kinds without
shadowing template-author code (`src/engine/audit.rs`). Putting typed event
types there would make one prefix denote two different layers of the log
format, which is worse than either choice alone.

A distinct namespace also does real disambiguation work. After this, grepping
`request_store` returns only dispatch-protocol things — the event type, the
reserved kind prefix, the config table, the header field group — and the new
family is visibly a different thing.

### D5 — Naming: `request` for the container, `leg` for the member

**Decision:** The container is a **request**. Its addressable member is a
**leg**.

**Why not `slot`.** In the corpora a reader or an agent has actually been
trained on, a slot is fungible capacity, not an identity. Open MPI defines a
slot as an allocatable unit bounding how many processes may run without
oversubscribing. Airflow pools ship a default pool of 128 slots that cap
parallelism. Grid Engine queue slots default to the core count, one process per
slot. In all three a slot has no identity, no result, and is *reused* when its
occupant finishes — the exact opposite of a member that is individually
addressable, accepts one result ever, is individually abandonable, and is never
reused. A `--slots 5` flag reads as "let five run at once," which is a
wrong-behavior misread rather than a style quibble. Nor is `slot` what
comparable systems call a fan-out member: Slurm says array task, Kubernetes
says completion index, test runners say shard.

**Why not `task`.** It is koto's own word for a batch member, with a validated
name grammar this PRD reuses (R8). But per D2 a batch task is
scheduler-spawned with `needs_agent: None` and outside the dispatch fence,
while a leg is agent-dispatched by another process. Reusing the word would give
it two meanings on one substrate — the failure D2 exists to avoid.

**Why `leg`.** It reads correctly for a parallel branch of a fan-out, and it
collides with nothing in koto's vocabulary. Considered and set aside: `part`
(no collision, but says nothing), `index` (only right if members are
positional, and R8 makes them named), `arm` (good parallel-branch metaphor,
unusual in a CLI).

**Why the container is a `request`.** The config namespace and the header field
group already say it, and the ambiguity that would motivate renaming it is
better fixed by D4's namespace split than by a second rename. Considered and
set aside: `ask` and `inquiry`, both unused in koto and genuinely unambiguous —
stronger on legibility alone, rejected because D4 removes most of the pain and
because a rename would orphan the existing config and header vocabulary.

**What this does not fix.** "Request" still denotes two things: the first-class
object here, and the `needs_agent` header flag that the config table, an
operator-facing error string, and the koto-user skill all call a request. D1's
table is the reader's guide, and R68 requires the skills be corrected in the
same change. A prose convention alone would not hold, which is why the durable
fix is D4's namespace split.

### D6 — Progress is a repeatable leg append, not a new progress event and not `IntentUpdated`

**Decision:** Mid-flight progress is an ordered, append-only sequence on a leg
(R22–R24). There is no dedicated progress event type, and `IntentUpdated` is
not repurposed (R25).

**Alternatives:** (a) progress as leg appends; (b) a dedicated
`ProgressReported` event; (c) extend `IntentUpdated` or its single-field
last-write-wins semantics to carry progress.

**Why.** "Here is an intermediate artifact" and "here is a progress note" are
the same operation from the substrate's point of view. Splitting them would
make a coordinator read two sequences and interleave them to recover one
timeline, for no gain.

`IntentUpdated` is the wrong vehicle for the opposite reason: it is one field,
free text, last write wins. Progress is many entries, structured, and
order-bearing. Overloading a last-write-wins field with an append-only sequence
would either lose entries or quietly change what `--intent` means for every
existing caller.

**The teachable distinction**, which R68 requires the koto-user skill to carry:
`--intent` is one sentence about what your whole workflow is for and each write
replaces the last; a leg progress append is one entry about what you just
finished for the request someone asked you to fill, and every entry is kept in
order.

### D7 — The operation is `abandon`, and it is cooperative rather than a cascade

**Decision:** Abandoning a leg or request stops the request waiting and records
why; it does not cancel the bound delegate's workflow (R29). The delegate learns
through koto's existing directive dispatch and can stop (R30). The verb is
`abandon`, not `cancel` (R28).

**Alternatives on semantics:** (a) cooperative — request-side stop plus a signal
the delegate acts on; (b) cascade — abandoning a leg cancels the bound child's
workflow; (c) request-side only, with no signal to the delegate.

**Why not cascade.** A child's workflow is its own audit object with its own
terminal semantics and often its own consumers. Letting one coordinator's
bookkeeping terminate it means a request-level decision destroys a
workflow-level record, and a child dispatched for two purposes could be killed
by whichever coordinator gave up first.

**Why not request-side only.** koto has no process control, so a leg whose
delegate keeps working is real waste — the delegate burns effort on an answer
nobody will read. Request-side-only makes that waste invisible and unavoidable.

**Why the verb changed.** In comparable tools, `cancel` stops the worker:
`scancel` signals and terminates a job's steps, `gh run cancel` escalates to
killing the process tree, deleting a Kubernetes Job deletes its Pods by
default. A coordinator that learned the word there will believe it freed the
delegate's budget. It did not. The one widely used system with exactly these
semantics avoids the word: Celery calls it **revoke** and documents that a
revoked task already executing is not terminated unless a separate terminate
option is set; Kubernetes' equivalent, orphaning dependents, is likewise an
opt-in flag rather than the default. `abandon` says what happens — the
requester stops waiting — and leaves `cancel` meaning what it means everywhere
else, including in `koto cancel`. It also makes R31's no-accidental-reachability
structural rather than guarded (R43).

**The remaining honesty.** A delegate that never checks in again keeps working
on an abandoned leg until it finishes. R30 narrows the exposure by requiring
the signal arrive through the dispatch path an agent already follows rather than
a field it must be taught to poll — otherwise the cooperative model degrades
silently into the request-side-only alternative this decision rejected.

### D8 — Readiness lives in a wait operation; the read exits zero

**Decision:** The request read exits zero whenever the fetch succeeded and
carries readiness in a discriminated payload field (R34). A separate wait
operation owns readiness, takes its predicate and timeout explicitly, and is
the only one of the two exiting non-zero for an unmet condition (R36, R37).
Exit statuses come from koto's documented four classes with no new numbers
(R47).

**Alternatives:** (a) read exits zero with a payload discriminator, plus a
separate wait verb owning the readiness status; (b) readiness as a non-zero exit
on the read; (c) readiness only in the payload, with no wait verb at all.

**Why.** `koto next` already answers "the thing you are waiting for has not
happened yet" by exiting zero with `action: "gate_blocked"`, and
`docs/prds/PRD-koto-next-output-contract.md` states the rule outright: every
success shape is exit zero, and the only loop control a caller needs is whether
`action` is `done`. There is a gate-blocked error code mapped to exit 1 in
`NextErrorCode`, but it is dead — defined, mapped, and unit-tested, constructed
nowhere in the crate. So the koto surface a shell consumer already polls exits
zero while waiting, and specifying the opposite here would hand one consumer
two incompatible polling idioms, one of which dies under `set -e`.

The convention outside koto points the same way. kubectl, the AWS CLI, gh, and
docker all keep readiness out of the read verb and put it in a dedicated wait or
watch — and gh makes even the watch verb's failure status opt-in behind a flag.
A read exiting non-zero also breaks every non-polling consumer: the auditor, the
restart path, the dashboard, all of which fetch a request that is legitimately
still open.

Alternative (c) is insufficient for the opposite reason: a shell loop that
cannot branch on status has to parse, which is the fragility a versioned
envelope exists to remove. Splitting the verbs also fixes something a single
exit code could never express — for a multi-leg request, "is it ready" has no
single answer, so R36 makes the predicate an argument and lets "three of five is
enough" be stated rather than guessed.

### D9 — One noun group, `koto request <verb>`

**Decision:** Every operation lives under a single `koto request` noun group,
with the request identifier first positional and the leg name second (R42).

**Why.** koto's convention is unambiguous once the command enum is read: the
workflow is the implicit primary object and gets top-level verbs — init, next,
cancel, rewind, status — while every other object is a noun group: template,
session, context, decisions, overrides, config, workspace. A request is not a
workflow, so the noun group is the native shape, and `koto context add <session>
<key>` is exact precedent for container-then-member positionals.

Putting request abandonment on `koto cancel` would make it the only koto verb
acting on two different object types, and the guard R31 demands would have to be
enforced inside one argument grammar. Separate grammars make the property
structural instead (R43).

### D10 — Explicit result recording is for unbound legs only

**Decision:** A bound leg resolves by promotion from its child's terminal result
and rejects an explicit result. The explicit resolve operation applies only to
legs with no bound child.

**Why.** Automatic promotion and an explicit resolve operation both exist, and
both produce the same envelope shape, so without a rule an agent cannot tell
which to call. The concrete failure is worse than confusion: a conscientious
delegate records its verdict explicitly, then completes normally, and the
promotion path hits a leg that already has a result. Under the
at-most-one-result rule that is a rejection, and if the rejection surfaces on
the child's own terminal tick, the delegate is stranded unable to complete.
Symmetrically, a coordinator resolving on a slow delegate's behalf would
permanently block the real answer.

Binding is the discriminator because it is already what decides whether a leg
has a worker of its own. The rule is one sentence an agent can hold: if
something else is doing the work, let it report; if nothing is, report it
yourself.

### D11 — For a bound leg, the coordinator's own directive stays the canonical result read

**Decision:** The request view is the surface for progress, partial state,
cross-session reads, and restart recovery. For a bound leg's final result, the
coordinator's own `koto next` directive remains canonical (R63).

**Why.** The koto-user skill states in several places that a coordinator reads
what its children produced inline from its own gate output and must never tick
or query the child. This PRD makes the same result legitimately readable a
second way, and for the one-leg case R62 guarantees the two overlap. Without a
precedence rule an agent either polls the request view while its own gate is
what actually advances its workflow — so it never advances — or reads the gate
and never notices progress. The skill's canonical-source table has to stay
single-valued, which is why R68 requires it be updated rather than left to
drift.

### D12 — Complexity: Complex, routes to design before planning

**Decision:** Classified Complex. A technical design comes before decomposition.

**Why.** The feature adds a first-class object and six event variants to koto's
closed event enum, reintroduces a request identity an earlier design
deliberately collapsed away, adds a second CLI noun group, and touches the claim
path, the discovery scan, and the terminal index — the hardened,
crash-safety-critical parts of the substrate. It also leaves six genuine
architectural choices open on purpose, enumerated under Deferred to Design. None
is a localized change with one obvious implementation.
