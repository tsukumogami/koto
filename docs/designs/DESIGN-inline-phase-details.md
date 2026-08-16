---
schema: design/v1
status: Proposed
upstream: docs/prds/PRD-inline-phase-details.md
problem: |
  koto decides whether a phase's long-form instructions ride in a `koto next`
  response by counting entries into a state. The quantity that matters is
  deliveries of that phase's instructions, and the two come apart in both
  directions: a non-advancing tick enters nothing so the instructions re-send
  forever, while a rewind is an entry so they are withheld from an agent told to
  redo the phase. The directed-transition path applies no rule at all. Beneath
  that, no log-derived rule can tell an agent that still holds the instructions
  from a compacted or respawned one, and koto has no read-only way back to them.
decision: |
  Record the fact of delivery rather than infer it from movement. A new
  `InstructionsDelivered` event is appended when a response carries a phase's
  instructions, and the suppression predicate becomes "has a delivery been
  recorded since the most recent entry into this phase". A shared combinator on
  the response type applies that predicate at both response-construction sites,
  so the directed path and the natural-advancement path follow one rule. For the
  case no predicate can reach, `koto status` gains the current phase's
  substituted directive, instructions, and evidence schema -- it is already
  read-only, already loads the compiled template, and takes no lock. A short
  koto-authored pointer is spliced into the directive, reusing the mechanism that
  already carries the leg-abandonment notice, so an agent that lost everything
  else still learns the recovery call exists.
rationale: |
  The defect is that koto counts the wrong thing, so the fix is to count the
  right thing rather than to tune the wrong one. Recording a delivery costs one
  event and makes rewind, self-transition, directed transition and multi-hop
  auto-advance all fall out without special cases, because the predicate keys on
  position relative to the last entry event and every one of those paths appends
  one. Deriving delivery from existing events was investigated and disproved: two
  sessions that differ only in whether instructions were delivered produce
  identical logs. Extending `koto status` rather than adding a command avoids
  introducing the CLI's first use of "phase", a noun deliberately confined to the
  `/workflows` translation layer, and reuses a handler that already satisfies the
  no-side-effects contract by construction.
---

# DESIGN: phase instructions an agent can rely on

## Status

Proposed

Authored under `/scope`'s tactical chain from `PRD-inline-phase-details`. Four
decisions were evaluated independently and cross-validated; the cross-validation
surfaced a contradiction in the PRD's own acceptance criteria, which was
corrected upstream before this document was written.

## Context and Problem Statement

koto lets a template attach long-form instructions to a phase by splitting the
phase body on an HTML comment marker, and `koto next` decides per response
whether to include them. Today that decision is `full || visit_count <= 1`, where
the count comes from `derive_visit_counts` (`src/engine/persistence.rs:981`)
counting how many times a phase name appears as the `to` field of a
`Transitioned`, `DirectedTransition`, or `Rewound` event.

That predicate answers "how many times has the workflow entered this phase". The
question the feature needs answered is "have I already handed these instructions
to whoever is asking". The two diverge, and the divergence is not a corner case.
A tick that evaluates gates, fails them, and does not transition appends no entry
event, so the count never moves and the instructions ride every response for as
long as the agent stays blocked. A rewind appends an entry event into a phase the
log has necessarily entered before, so the count is already past the threshold
and the instructions are withheld from an agent explicitly being told to redo
that phase. And the directed-transition path never evaluates the predicate at
all: `dispatch_next` (`src/cli/next.rs:50-54`) sets the field whenever the phase
declares one.

Underneath sits a failure no predicate over this log can reach. The session log
records where the workflow has been, never who is attached to it. A cold-restart
respawn continues on its predecessor's log, so a zero-context agent inherits a
delivery it never received; context compaction leaves no event at all. The
payload in question is a tool result, which the platform documents as
compaction-eligible. So suppression can only ever be an optimization, and its
safety has to come from a way back to the instructions. koto has none: `koto
status` is read-only but carries neither the directive nor the instructions, and
`koto next --full` carries them while also evaluating gates, appending events,
re-executing any `default_action` shell command, potentially auto-advancing, and
potentially cleaning up a terminal session.

## Decision Drivers

- **The rule must be one rule.** Two response-construction sites disagreeing is
  what makes the documented contract false today (PRD R4).
- **No new state file, no schema-version bump** (PRD R16). This is R9 of
  `PRD-koto-next-output-contract` carried forward, and it eliminated a persisted
  counter file when that requirement was first written.
- **No new file reads on the `koto next` path** (PRD R18).
- **A phase declaring no instructions must behave byte-identically to today**
  (PRD R6). This constrains the discoverability pointer as much as the predicate.
- **The recovery call must have an exhaustively checkable set of non-effects**
  (PRD R11) and must not block on another process's lock (PRD R12).
- **`derive_visit_counts` has a second consumer** at
  `src/workflows_surface/project.rs:284-286` for an unrelated visited-set
  purpose, so its semantics are a constraint, not a target.
- **koto's CLI speaks "state".** The engine and CLI use it overwhelmingly;
  "phase" is a deliberate translation confined to `src/workflows_surface/` so the
  `/workflows` render matches Claude Code's own noun. A new CLI surface named
  "phase" would be the first crossing of that boundary in the wrong direction.

## Considered Options

Four decisions were decomposed and evaluated independently, each against its own
alternatives. Their substance is recorded below; the working reports do not
survive the branch, so nothing here defers to them.

### Decision 1 — what records a delivery

**A. A new `EventPayload::InstructionsDelivered { state }` variant.** Appended
when a response carries the instructions. The predicate becomes "is there a
delivery record after the most recent entry event for this phase".

**B. An `EvidenceSubmitted` event using the reserved dotted pseudo-state
convention** in `src/engine/audit.rs`, the same trick `request_store.respawn` and
the abandonment notice use. No new variant, no schema question at all.

**C. An additive `StateFileHeader` field.** Rejected on a compile-time fact: the
header is pinned by `koto-stability-tests`, and adding a field breaks PRD R17.

**D. Record nothing; derive delivery from events that already exist.** This was
the option worth wanting, and it is disproved rather than merely disfavoured. A
non-advancing tick on an `accepts`-only phase awaiting evidence appends nothing
at all, so two sessions differing only in whether instructions were delivered
produce byte-identical logs. No predicate can distinguish states that are
identical.

**Chosen: A.** With D impossible and C eliminated, the choice is A against B, and
both have the same write cost and the same compatibility profile. A wins on
honesty and on total code: B needs a reserved kind, a synthetic state, an
`is_reserved_kind` extension, a reservation test, and filters in the
`/workflows` projection and the dashboard timeline — and after all of it the
record still claims to be evidence submitted in a phase that does not exist. A
needs a variant, a `type_name` arm, a deserialize arm, a payload struct, and a
doc comment, and every existing projection handles it through an arm already
present.

Adding a variant is not a schema change: `CURRENT_SCHEMA_VERSION` stays at 1
under the rule its own doc comment states, and six `request.` variants shipped in
one change without moving it.

### Decision 2 — where the read-only retrieval lives

**A. Extend `koto status`.** `handle_status` (`src/cli/mod.rs:4834-4960`) already
reads the events, loads and parses the compiled template, and looks up the
current phase to compute `is_terminal`. It writes nothing and takes no lock.

**B. A new top-level subcommand,** named `phase-info` as the source issue
proposed, or `state-info` to stay inside the CLI's vocabulary.

**C. `koto next --dry-run`.** Rejected, and by a hard constraint rather than a
preference: D1 established that anything routing through `handle_next` appends
`GateEvaluated`, executes default actions, rewrites the discovery cursor, and can
call `finish_terminal_tick`. A flag cannot subtract those. R10 and R11 require a
surface with its own handler.

**Chosen: A, with no flag.** `status` gains `directive`, `details`, and `expects`
as conditionally-present keys, following the "present only when relevant"
convention it already uses for `batch`, `leg`, `superseded_branches`, and
`stale_template_source_dir`. No flag, because `status` has no notion of first
visit to key one off, and it is a spot-check command rather than a per-tick poll,
so the cost of the extra fields is paid only when someone reaches for recovery.
This introduces no new noun at all, which disposes of the naming question rather
than answering it.

### Decision 3 — unifying the two construction sites

**A. Thread the check into `dispatch_next`.** **B. Rewire the `--to` handler to
share the advance path's computation.** **C. A shared combinator on the response
type, applied at both post-construction call sites.** **D. Collapse the two
construction sites into one.**

**Chosen: C**, as `NextResponse::with_details_suppressed_unless_full(self,
already_delivered, full)`, sitting beside the existing `with_substituted_directive`
and `with_directive_prefix` and called at `src/cli/mod.rs:3357` (directed) and
`src/cli/mod.rs:4198` (natural). It is the smallest true unification: `dispatch_next`
keeps its signature and its twenty existing tests, the main path's per-arm
`details.clone()` duplication collapses into one call, and the idiom is not new —
it is the third instance of "compute once, transform every variant uniformly,
apply at the call site" that the response type already uses for `directive`.
A and B both force a `dispatch_next` signature change for no behavioural gain.
D is a larger refactor than R4 asks for, and the one change that would make it
clean — evaluating gates on the directed path — is explicitly out of scope.

### Decision 4 — the discoverability pointer

**A. Reuse `with_directive_prefix`,** the mechanism already splicing the
leg-abandonment notice into `directive` after substitution. **B. A new structured
sibling field.** **C. Carry it in `expects`.** **D. No per-response pointer; rely
on the koto-user skill's standing instructions.**

**Chosen: A, with D as a required complement rather than an alternative.** The
mechanism exists and is already exercised for a structurally identical problem:
koto-authored text that must reach the agent through the field it treats as
authoritative, on every applicable tick, without touching `details` and without
needing the variants that carry no directive. Its coverage is exactly the five
instructions-bearing variants R14 names, by construction rather than by a
parallel check that could drift.

The pointer is held under roughly 150 characters. That discipline is the whole
cost: a sub-100-character pointer is on the order of one percent of what a
fourteen-tick suppression run saves, while one written at the abandonment
notice's length would be closer to eight.

D is a complement because the skill documentation R20 and R21 already mandate is
a second, independent channel — one that lives in the agent's system context
rather than in a tool result, and therefore fails differently.

## Decision Outcome

The four decisions compose into one change with a single organising idea:
**record the fact of delivery instead of inferring it from movement.**

Recording is what makes the rest fall out. The predicate keys on position
relative to the most recent entry event, and every way of arriving at a phase —
conditional transition, unconditional transition, directed transition,
self-transition, rewind, initialization — appends one. So each of those starts a
fresh occupancy with no special case in the predicate, which is precisely the
uniformity R3 and R4 ask for and precisely what a visit count cannot give.

The shared combinator is what makes it one rule rather than two implementations
of one rule. The `koto status` extension is what covers the case the predicate
provably cannot: the log has no notion of who is attached, so a respawned or
compacted agent is indistinguishable from one that already holds the
instructions. The pointer is what makes that coverage reachable by an agent that
has lost the knowledge that it exists.

Three consequences of this composition are worth stating plainly rather than
leaving to be discovered.

**A non-advancing `koto next` will write where it may not today.** On a phase
that declares instructions and is receiving them, koto appends a delivery record.
The existing `if details.is_empty()` guard keeps both the extra read and the
extra write entirely off instruction-free phases, which is what preserves R6 and
R18. This is the sharpest cost of the design and it is accepted deliberately: the
alternative was disproved, not merely disfavoured.

**PRD R18's acceptance criterion is stricter than R18 itself.** The requirement
is about reads; the delivery record is an additional append to a file the tick
already opened. The criterion was corrected upstream to exclude writes, and the
verification step should be run against reads only.

**A contradiction in the PRD was corrected.** Its Definitions made a
self-transition begin a new occupancy — so instructions must be delivered — while
an acceptance criterion required a second consecutive directed transition into
the same phase to omit them. Those two are only jointly reachable when a template
declares a self-transition, since the directed handler validates its target
against the current phase's declared transitions (`src/cli/mod.rs:3304-3322`),
and that path appends `DirectedTransition { from: X, to: X }`, which is a new
occupancy by the PRD's own definition. The Definitions are normative and R3 is
explicit, so the criterion was rewritten to test what it was plainly reaching for:
a directed transition followed by a non-advancing tick.

## Solution Architecture

### Components and where they change

| Component | File | Change |
|---|---|---|
| Event model | `src/engine/types.rs` | `EventPayload::InstructionsDelivered { state }`, its `type_name()` arm (`"instructions_delivered"`), its deserialize arm, a payload struct, and a doc comment in the existing house style explaining additive safety |
| Delivery predicate | `src/engine/persistence.rs` | `instructions_delivered_this_occupancy(events, state) -> bool`, placed beside `latest_epoch_gate_failed` and sharing its slicing idiom |
| Response combinator | `src/cli/next_types.rs` | `with_details_suppressed_unless_full(self, already_delivered, full)`, beside `with_substituted_directive` and `with_directive_prefix` |
| Pointer splice | `src/cli/next_types.rs` | Reuse `with_directive_prefix`, applied after substitution, when the phase declares instructions |
| Natural path | `src/cli/mod.rs` (~3999-4016, 4198) | Replace the `derive_visit_counts` / `count <= 1` check with the predicate; call the combinator; append the record when the response carries instructions |
| Directed path | `src/cli/mod.rs:3355-3357` | Call the same combinator; append the same record |
| Retrieval | `src/cli/mod.rs` `handle_status` | Add `directive`, `details`, `expects` as conditionally-present keys |

`derive_visit_counts` stays. This design stops using it for the delivery
question; it does not remove it, and its `workflows_surface` consumer is
untouched.

### Data flow, natural-advancement path

1. `handle_next` runs `advance_until_stop` as today, which appends whatever entry
   and gate events the tick produces.
2. The post-advance event list is read once — the same read the tick already
   performs — and `instructions_delivered_this_occupancy` is evaluated against
   the phase the loop stopped at.
3. The response is constructed per `StopReason` as today, then passed through
   `with_substituted_directive`, then the new combinator, then the pointer
   splice.
4. The response is printed. Then, if it carried the instructions, the delivery
   record is appended.

Step 4's ordering is deliberate. A crash between printing and appending
re-delivers on the next tick, which is the benign direction: an agent receives
instructions it already had, rather than being denied instructions it never got.

### Data flow, directed path

Identical from step 3, with one difference at step 2: the event list is built in
memory from the payload the handler has already appended rather than re-read from
disk, which is what holds R18 on this path. The predicate takes a plain
`&[Event]` slice with no backend coupling, which is what makes that possible.

### `koto status` output

Three conditionally-present keys are added:

```json
{
  "name": "...",
  "current_state": "...",
  "template_path": "...",
  "template_hash": "...",
  "is_terminal": false,
  "directive": "<substituted>",
  "details": "<substituted, absent when the phase declares none>",
  "expects": { }
}
```

`directive` and `details` come from `compiled.states[current_state]` and go
through the same two-layer substitution `next` uses, so recovered text is
identical to what `next` would have produced. `expects` comes from
`derive_expects`. All three are absent when the phase is terminal, matching the
terminal response variant's existing behaviour of carrying no directive, which
gives R13's "terminal is a normal success response" a concrete shape. `details`
is absent when the phase declares none, which is the other half of R13.

`handle_status` acquires no lock — the only `lock_state_file` call site is inside
`handle_next`, gated to batch-parent phases — so R12 holds by construction rather
than by a new guarantee.

### The one thing the pointer must not key on

The pointer's presence condition is "this phase declares instructions", not
"instructions are in this response". Those differ on exactly the responses where
the rule suppressed the instructions, which are the responses a recovering agent
most needs the pointer on. The pointer must not reuse the delivery predicate for
this.

### Splice ordering when both notices apply

`directive` already carries one koto-authored splice: the leg-abandonment stop
notice. Both it and the recovery pointer can apply to the same response — an
abandoned leg on a phase that declares instructions — so their order has to be
stated rather than discovered.

**The recovery pointer is spliced first, the abandonment notice second, so the
abandonment notice ends up closest to the front of `directive`.** The reason is
that the two say different kinds of thing. The abandonment notice tells an agent
to stop; the pointer tells it where to look something up. Burying a stop
instruction underneath routine navigational text would defeat the notice, and the
notice exists precisely because the agent must act on it before anything else.
Ordering them the other way costs nothing when only one applies and protects the
more urgent one when both do.

## Implementation Approach

Four phases, sequenced by what each needs from the one before.

**Phase 1 — the record and the predicate.** Add the event variant and
`instructions_delivered_this_occupancy`, with unit tests over synthetic event
lists covering: no prior delivery, a delivery in the current occupancy, a
delivery before the most recent entry event, a rewind entry, a self-transition
entry, and a multi-hop advance where the delivery belongs to an intermediate
phase. Nothing observable changes yet.

**Phase 2 — the combinator and both call sites.** Add
`with_details_suppressed_unless_full`, wire the natural path to the new predicate,
and wire the directed path to the same combinator and the same append. This is
where the behaviour changes, and where the byte-identity baseline for
instruction-free templates must already have been captured.

**Phase 3 — the retrieval.** Extend `handle_status`. Independent of phases 1 and
2 except that its output shape should match what `next` produces, so it lands
after them.

**Phase 4 — the pointer, then the documentation and evals.** The pointer needs
the retrieval's exact invocation to name, so it follows phase 3. The skill,
guide, Cursor-rule and eval updates that PRD R20 through R25 require land with
it.

The coupling that matters is inside phase 2, not between phases 1 and 2. Phase 1
is inert by construction — it adds a variant and a predicate that nothing calls —
so it can land on its own. But phase 2's two halves, wiring the natural path and
wiring the directed path, must land together: shipping one without the other
leaves the two paths disagreeing, which is the defect R4 exists to close, and
would replace one inconsistency with a different one. Phases 3 and 4 could be
separated, but phase 4 without phase 3 points at nothing.

## Security Considerations

This change moves author-supplied template text — the same `directive` and
`details` that `koto next` already substitutes and returns — onto a second,
side-effect-free read path, and adds one append-only event variant. Neither
introduces a new trust boundary. That text was already fully controlled by
whoever controls the template file, and it already reached the agent through
`koto next`. `koto status` reading the same fields through the identical
substitution pipeline does not widen who can influence the text or where it
lands. Substitution on this path never enters a shell-command or filesystem-path
context; that risk is confined to the `substitute_command` call sites this design
does not touch.

The new event variant is safe under koto's existing compatibility contract
without any additional work. The custom `Event` deserializer routes any
unrecognized event type to `Unknown` through an unconditional catch-all, so an
older binary round-trips the new record unharmed, and a crash mid-append produces
the same recoverable malformed-final-line case any other event type already
produces on a torn write. The added write is bounded by occupancy count rather
than tick count, so the design's own motivating case — a long gate-blocked loop —
adds one record, not one per iteration. Because the predicate is an existence
check, the unlocked concurrent-append race on a non-batch session produces at
worst a harmless duplicate record, never a wrong answer.

The directive splice follows the same after-substitution ordering the existing
leg-abandonment notice established, which is the property that matters here:
koto-authored text is never subject to re-substitution, and template-authored
text can never reach into or alter it, in either direction. `koto status` is
unlocked by construction rather than by conditional — `lock_state_file`,
`append_event`, gate evaluation and terminal cleanup are simply absent from its
call graph — so the read-only guarantee this feature leans on holds structurally.

One finding needs an explicit ruling rather than silent inheritance. `koto
status` reads the compiled template without verifying its hash against the value
recorded in the session header, which `koto next` does verify. That gap predates
this change, and today it affects only the `is_terminal` boolean. Once `status`
returns substituted instructions off the same unverified read, a stale or
mid-replacement template would surface as fully-formed recovered instructions
rather than as a quietly wrong flag — and the caller is, by construction, an
agent that has lost its context and has no way to notice.

**The ruling: `handle_status` verifies the hash, and reports a mismatch rather
than failing on it.** Failing would deny an agent its instructions at exactly the
moment it has nothing else, which is the failure this feature exists to prevent.
Instead, a mismatch adds a conditionally-present key naming the divergence,
following the same "present only when relevant" convention `status` already uses
for `stale_template_source_dir` — a signal the caller can act on, alongside the
best-effort content. The pre-existing `is_terminal` gap is closed by the same
check as a side effect.

Nothing else becomes reachable that a caller could not already obtain. `details`
was already returned by `koto next`, `directive` is already exposed through the
`/workflows` projection and the dashboard, and the template file `status` already
names in its output is ordinarily repo-committed content the caller can read.

## Consequences

### Positive

- One predicate governs every arrival path, so the contract koto's own skills
  document becomes true.
- The rewind case inverts from its worst behaviour to its correct one: an agent
  told to redo a phase gets that phase's procedure.
- The recovery path exists at all, and it exists on a handler that already
  satisfies its non-effects contract rather than one that has to be argued into
  satisfying it.
- No new CLI noun, so the `state`/`phase` boundary stays where it was
  deliberately drawn.
- `derive_visit_counts` and its second consumer are untouched.

### Negative

- A non-advancing `koto next` on an instruction-bearing phase now appends an
  event. Log growth over a long loop is bounded by one small record per delivery,
  not per tick — deliveries are what the feature suppresses — but the write is
  real and new.
- The event enum gains a variant, which is public surface even though it costs no
  schema bump.
- `koto status`'s output grows for the common case, since most phases carrying
  instructions are not terminal. It is a spot-check command, not a per-tick poll,
  so this is paid rarely.
- A crash between printing a response and appending its delivery record causes
  one redundant re-delivery.

### Mitigations

- The `details.is_empty()` guard keeps every new read and write off phases that
  declare no instructions, which is what makes R6's byte-identity claim
  achievable rather than aspirational.
- Ordering the append after the print makes the crash window fail toward
  re-delivery rather than toward silent suppression.
- The pointer's length is capped by convention and checked in review; the design
  states the budget explicitly so it does not drift.
