---
schema: design/v1
status: Accepted
upstream: docs/prds/PRD-self-loop-suppresses-details.md
problem: |
  koto decides whether a response carries a phase's instructions by asking
  whether a delivery has been recorded since the phase was last entered, and it
  counts a phase transitioning to itself as an entry. An agent looping inside a
  phase is re-sent a procedure it already holds on every lap. The boundary the
  decision reads is shared with the gate-blocked classification the dashboard
  shows, and with three open-coded copies elsewhere in the same file, so moving
  it naively moves things the change has no business moving.
decision: |
  Split the boundary in two and give each half a name. One private scan,
  parameterized by which entry events close its window, is exposed through
  `epoch_slice` -- behaviorally identical to today's helper and backing the gate
  classification -- and `delivery_window`, which skips an entry whose source
  phase equals its target. No call site names a boundary. The rewind arm of the
  match does not bind the source phase at all, so a rewind opens both windows and
  the delivery answer cannot come to depend on how `koto rewind` picks its
  destination. The accepted PRD and current DESIGN that state the old boundary
  are amended in place, and the passage that argued for it is rewritten into a
  record of both rulings.
rationale: |
  The delivery decision and the gate classification now answer different
  questions, so they must read different boundaries; the shipped code says the
  opposite in a comment, and that comment has to be overturned deliberately
  rather than quietly. Duplicating the scan would give the file a fifth copy of a
  walk it already carries four of, and a boundary argument at the call sites
  would put a silent behavior switch where a typo reaches it. One scan and two
  names costs a rename and buys a unit test that reads as one log through two
  functions with opposite answers, which is exactly what the requirement says.
---

# DESIGN: two boundaries, two names

## Status

Accepted

Authored under `/scope`'s tactical chain from
`PRD-self-loop-suppresses-details`. Three decisions were evaluated
independently and cross-validated. Two of the three decision-researcher agents
died on transient API errors; those two questions were analysed directly by the
design author against the source, and each report says so at its head.

## Context and Problem Statement

koto includes a phase's long-form instructions in a `koto next` response when no
delivery of them has been recorded since the workflow entered that phase. The
scan that answers "since the workflow entered that phase" is `occupancy_slice`
(`src/engine/persistence.rs:1028`): a backwards walk for the most recent
`Transitioned`, `DirectedTransition` or `Rewound` whose target is the current
phase, returning everything after it.

A phase that transitions to itself appends such an event, so the scan restarts
and the instructions go out again. `PRD-self-loop-suppresses-details` requires
that they do not: an agent going around a loop it is already inside has the
procedure, and koto#90 said so before the mechanism was built.

The problem is not finding a rule that suppresses. It is moving one boundary
without moving three others that happen to be spelled the same way:

- `latest_epoch_gate_failed` (`:1058`) calls the same `occupancy_slice`. It is
  the blocked classification the dashboard (`src/cli/dashboard_data.rs:458`) and
  the `/workflows` projection (`src/workflows_surface/project.rs:183`) both read,
  and it has no direct test coverage anywhere in the repo.
- `derive_evidence` (`:722`), `derive_overrides` (`:796`) and
  `derive_last_gate_evaluated` (`:844`) each open-code a similar backwards walk
  rather than calling the helper — and each already differs from it, returning
  empty where the helper returns the whole log when no entry event names the
  phase (`:743-746`, `:817-820`, `:860`, against `:1042-1045`). They also take no
  phase argument, deriving the current state themselves. They are neighbours of
  the helper, not callers of it.

`latest_epoch_gate_failed` takes the *latest* gate evaluation inside its slice,
so widening that slice changes its answer whenever the narrow epoch holds no gate
evaluation and the wide window does. That begins at the tick right after a
self-entry and lasts until a gate evaluation lands in the new epoch — not just
for one tick. Today the stale verdict drops out and the badge clears. Widened, a
failed gate from before the loop keeps the badge on "blocked", silently, with
nothing to catch it.

The shipped code argues against exactly the change this design makes. The helper's
doc comment reads: "Shared rather than copied so the predicates built on it -- the
epoch-scoped gate classification and the delivery check -- cannot come to disagree
about where an occupancy starts" (`:1017-1019`). That sentence was right when the
two predicates answered the same question. They no longer do, and overturning it
is part of the work rather than incidental to it.

Two upstream documents state the boundary as it is:
`docs/prds/PRD-inline-phase-details.md`'s Definitions section, normatively, and
`docs/designs/current/DESIGN-inline-phase-details.md`, which additionally carries
a passage headed "A contradiction in the PRD was corrected" resolving koto#90's
acceptance criterion 3 against delivery. That resolution has been reversed by the
issue's author. Leaving either document as it stands would reproduce the original
failure — a definition written down, believed, and never re-examined — in the
opposite direction.

## Decision Drivers

- **The gate epoch and the evidence epoch must not move.** PRD R15 states it and
  the dashboard's untested badge is why.
- **The delivery answer must not depend on how `koto rewind` picks a
  destination.** koto#199 is an open defect in that logic and is out of scope;
  PRD R6 is written so that whatever it changes, a rewind still delivers.
- **The failure direction must stay safe where it can.** The shipped design
  re-delivers when it is unsure — a lost delivery record reads as "not delivered".
  That bias survives everywhere except inside a loop that has already delivered,
  where the whole point is to stop, and one case that only a truncated log can
  produce: a log whose only entries naming a phase are self-entries falls through
  to the whole-log fallback, where a record from an earlier visit can suppress
  where the shipped helper would have delivered. It is not reachable through
  normal operation, because a phase's first entry is never a self-entry, and it
  is named here rather than left to be found.
- **No new event, no new field, no schema bump.** PRD R16. The change is read-side.
- **What the code claims about itself must stay true.** Two comments in the
  shipped tree become false under this change, one of them a 24-line proof. Both
  are load-bearing for a reader trying to understand why the code is shaped as it
  is.
- **Reviewability.** The diff has to be readable as one idea. A behavior reversal
  and a behavior-preserving refactor of three open-coded scans in the same commit
  would be neither.

## Considered Options

The three decisions were evaluated independently. Each is summarized here with
the evidence that settled it; the working reports are not durable and this
section is the record.

### Decision 1 — where the delivery-window boundary lives

**Option A, a sibling scan.** Add a second private function; leave the shared
helper alone. Zero risk to the gate epoch, and the smallest conceptual change.
Rejected on one margin: it takes the file from four backwards walks over entry
events to five, with the two that matter sitting adjacent and differing by one
line. The margin is slim — the chosen option still leaves four, three of them
open-coded — and the stale doc comment is not an argument against A, because
every option here rewrites it.

**Option B, a boundary-policy parameter.** Option D minus the two wrappers: same
enum, same inner scan, discriminator exposed at the call sites instead of hidden
behind names. Rejected because that is where the whole difference lands. A wrong
argument at `latest_epoch_gate_failed`'s call is a silent behavior change with no
type error, and the shared comment would have to read "shared so they cannot
disagree, parameterized so they do".

**Option C, widen the shared helper in place.** The cheapest thing that could
work. Rejected on measurement rather than taste: it is the change that moves the
dashboard's blocked badge across a self-loop, with no test in the repo to catch
it. PRD R15 exists because of this option, and its unit criterion is written to
fail loudly against it.

**Option D, rename and split over one shared inner scan.** Chosen. See Decision
Outcome.

### Decision 2 — how the reversal is recorded

**Option A, amend both documents in place.** Chosen. Commit `70ba97c` (PR #169)
is direct precedent: it amended five Current designs and three settled PRDs in
place against the real code, in one maintainer-authored commit whose message
says every fix was verified against `src/` and every changed doc passes
`shirabe validate`.

**Option B, supersede the DESIGN.** Rejected as disproportionate and lossy: the
design's four decisions — record delivery as an event, extend `koto status`,
share one combinator across both response paths, splice the recovery pointer —
all still describe shipped code, so a successor would restate every one of them
to move one clause, and the archive would hold the only copy of reasoning the
code still follows. It is also broken today. Executed against a throwaway copy,
`shirabe transition <design> Superseded --superseded-by <path>` exits 0, moves
the file to `docs/designs/archive/`, and writes a `## Status` body line reading
"Superseded by [...](...)" — which `shirabe validate` then rejects with an FC03
mismatch against the frontmatter status. Taking this option means shipping a
validation failure or hand-fixing the tool's output.

**Option C, a separate decision record.** Rejected because it fails PRD R18
outright: the accepted PRD's Definitions paragraph would still assert the old
boundary, and a reader arriving there has no reason to suspect a record elsewhere
contradicts it. koto also has no `docs/decisions/` directory, so this would
introduce an artifact class for one entry.

### Decision 3 — the baseline fixture's embedded prose

`tests/next_response_baseline.rs` generates a document compared as whole-string
equality against `tests/fixtures/next-response-baseline/instruction-free.json`,
and that document embeds the test file's notes and per-sequence descriptions. One
of those strings states the boundary this work moves: a sequence description
calling a self-transition "ending one occupancy and beginning another".

The sequence *label* on the next line reads `self-transition-arrival`, and it is
out of scope. A label is a slug naming a call sequence in a harness, not a claim
about what koto emits; it is asserted a third time by a hardcoded required-labels
list in the test file, so changing it is a three-site edit; and the PRD's
criterion permits only `notes` and `description` strings to move. Nobody reads a
test slug as documentation of the delivery rule.

**Option A, leave both alone** and carve the fixture out of R18 as test
scaffolding. Rejected: in the vocabulary the agent-facing docs use, "ends one
occupancy and begins another" is a statement that a self-transition re-delivers,
which is the thing R18 names. It is not a borderline case.

**Option B, lockstep edit to a corrected statement of the new rule.** Viable, and
rejected only against Option C.

**Option C, lockstep edit to wording that states topology and nothing about
delivery.** Chosen. The PRD is actively retiring the word "occupancy", and a
fixture that costs a two-file lockstep edit should not spend that cost again the
next time the vocabulary moves. A description that says `implement` transitions
to itself, and stops there, is true under any delivery rule.

Under every option the mechanism is a hand edit, never
`regenerate_baseline_fixture`. Regeneration cannot improve on a hand edit when
the implementation is correct — every recorded body comes back identical, by
R14 — and when the implementation is wrong it is the one action that converts the
failure this fixture exists to produce into a green test.

## Decision Outcome

**Two boundaries, two names, one scan.**

A private enum names which entry events close a window, a single private scan
takes it, and two named wrappers are what the rest of the file calls:

```rust
/// Which state-entry events close a scan window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// Every entry naming the phase. The gate and evidence epochs.
    AnyEntry,
    /// Only an entry from somewhere else. The delivery window.
    ArrivalFromElsewhere,
}

fn entry_slice<'a>(events: &'a [Event], state: &str, boundary: Boundary) -> &'a [Event];

fn epoch_slice<'a>(events: &'a [Event], state: &str) -> &'a [Event] {
    entry_slice(events, state, Boundary::AnyEntry)
}

fn delivery_window<'a>(events: &'a [Event], state: &str) -> &'a [Event] {
    entry_slice(events, state, Boundary::ArrivalFromElsewhere)
}
```

`latest_epoch_gate_failed` calls `epoch_slice` and behaves exactly as it does
today. The delivery check calls `delivery_window`. Neither names a boundary
value, so there is no argument at a call site for a future edit to get wrong.

The classification inside the scan is where the rewind exemption lives:

```rust
let opens = match &e.payload {
    EventPayload::Transitioned { from, to, .. } => {
        to == state
            && (boundary == Boundary::AnyEntry || from.as_deref() != Some(state))
    }
    EventPayload::DirectedTransition { from, to, .. } => {
        to == state && (boundary == Boundary::AnyEntry || from != state)
    }
    // A rewind opens both windows whatever it records. `from` is deliberately
    // not bound: the delivery answer must not depend on how `koto rewind`
    // chooses its destination (koto#199).
    EventPayload::Rewound { to, .. } => to == state,
    _ => false,
};
```

The rest of the scan is today's, unchanged: walk backwards for the last event
where `opens` holds, and

```rust
match start {
    Some(idx) => &events[idx + 1..],
    None => events,
}
```

The `None` arm is the whole-log fallback the shipped helper has and the three
neighbouring walks do not; a unit test pins it, and `epoch_slice`'s identity with
`occupancy_slice` depends on keeping both arms exactly as they are.

Not binding `from` on the rewind arm is the point rather than a detail. The
alternative — test `from != to` uniformly, then carve `Rewound` back out — reaches
the same answers today and is one deleted line away from coupling the delivery
rule to rewind's destination logic, in the file a koto#199 fix will be editing.
A field that is never read cannot be depended on by accident.

Initial entry falls out. `Transitioned.from` is `None` there, and
`from.as_deref() != Some(state)` is true for `None`, so initialization opens a
delivery window and the first tick delivers with no special case. A legacy log
missing the field reads the same way, which is the safe direction: an absent
field produces a delivery rather than a silence.

Two consequences of the split are worth stating rather than leaving to be found.

**The delivery rule stays keyed on the record, not on the entry event's shape.**
The window moves; the question asked inside it does not. A phase entered from
elsewhere whose delivery record was lost — a crash between printing a response
and appending its record, which the shipped design accepts — followed by a
self-transition, finds no record in the widened window and delivers. A rule that
suppressed on the entry event's shape alone would leave that agent with no
procedure and no lap that would ever hand it one. `PRD-self-loop-suppresses-details`
R1 is written for this reason and carries a unit criterion with no record in the
log at all.

**The directed path's correctness proof breaks, and that is the change.** The
comment at `src/cli/mod.rs:3377-3402` argues at length that the delivery check
"provably evaluates to `false` on every call" on that path, because the event it
just appended is always the newest element and therefore always the window
opener. Under `delivery_window` a `DirectedTransition { from: P, to: P }` is not
an opener, so the scan reaches back to the real arrival and finds its record. The
call becomes genuinely decision-bearing, which is what makes `koto next --to P`
at `P` suppress. The comment is replaced with the new argument, not trimmed.

## Solution Architecture

### Components and where they change

| Component | File | Change |
|---|---|---|
| Boundary scan | `src/engine/persistence.rs` | `Boundary` enum, `entry_slice`, `epoch_slice` (renamed from `occupancy_slice`, behavior identical), `delivery_window` (new) |
| Gate epoch | `src/engine/persistence.rs` | `latest_epoch_gate_failed` calls `epoch_slice`; behavior unchanged; doc comment says which boundary it reads and why it is not the delivery one |
| Delivery check | `src/engine/persistence.rs` | `instructions_delivered_this_occupancy` renamed to `instructions_delivered_this_window` and reading `delivery_window`; signature otherwise unchanged |
| Natural path | `src/cli/mod.rs` (~4283-4300) | Call the renamed check; rewrite the comment that states the boundary |
| Directed path | `src/cli/mod.rs` (~3366-3418) | Call the renamed check; replace the "provably false" proof with the new argument |
| Response combinator | `src/cli/next_types.rs` | No behavior change; doc comment states the delivery window rather than the occupancy |
| Event doc comment | `src/engine/types.rs` | Correct the `InstructionsDelivered` comment, which still says nothing appends the event |

`derive_evidence`, `derive_overrides` and `derive_last_gate_evaluated` keep their
own walks. Folding them into `epoch_slice` would not be a refactor at all: all
three return empty where the helper returns the whole log when no entry event
names the phase, so unifying them would change behavior on exactly the case a
unit test pins. Their independence is also, for now, a feature — PRD R15 pins the
evidence epoch, and a walk that shares no code with the delivery window cannot
follow it by accident.

`derive_visit_counts` stays untouched. It is the fossil of the rule the previous
change replaced, it still has a consumer in the `/workflows` projection, and
removing it is a separate change.

### Data flow

Unchanged from the shipped design on both paths. The natural-advancement path
reads the post-advance event list once — the read the tick already performs — and
evaluates the delivery check against the phase the loop stopped at. The directed
path builds its post-append list in memory from the payload it just appended,
which is what keeps it from performing a read the pre-change binary did not.
Neither adds nor removes a read.

The only difference is which events the check considers in scope, and that is
entirely inside `delivery_window`.

### Cost of the wider window

`delivery_window`'s slice can be longer than `epoch_slice`'s — for a loop with N
laps it spans the whole loop rather than the last lap. The check is
`.iter().any(..)`, and the delivery record for the arrival that opened the window
sits within the first handful of events after it, so the scan short-circuits
immediately regardless of how long the loop has run. The unbounded case is a
window with no record at all, which is the crash case, and it costs one pass over
events the tick has already loaded.

### Documentation surfaces

The rule is stated, in prose an agent or an author reads, in nine places:
`plugins/koto-skills/skills/koto-user/references/response-shapes.md` (four
passages plus an embedded example response whose `details` value states the rule
in its own text), `.../command-reference.md`,
`plugins/koto-skills/skills/koto-author/SKILL.md`,
`.../references/template-format.md`, `plugins/koto-skills/.cursor/rules/koto.mdc`,
`docs/guides/cli-usage.md`, `docs/reference/session-feed.md`, the koto-user eval
suite's explanations of the two evals PRD R20 protects, and the comments inside
`tests/instructions_delivery_test.rs` that narrate the rule alongside each
assertion. Most share a stock phrase — "leave and re-enter the state" — which
becomes exactly backwards, because leaving and re-entering *the same* state is
now the case that does not re-deliver.

Every one is rewritten to state the rule as an arrival test: instructions arrive
when you get to a phase from somewhere else, or when you are rewound into it, and
not when you go around a loop you are already in. That phrasing is what a template
author can apply to their own template without reading the engine, and it happens
to be correct on the case a paraphrase most often gets wrong — a tick that leaves
a phase, passes through another, and comes back within the same tick still
delivers.

`docs/reference/session-feed.md` and the `InstructionsDelivered` doc comment in
`src/engine/types.rs` additionally still say the event is not emitted yet. That
was true when they were written and false when the wiring landed in the same
change. Both are corrected here.

## Implementation Approach

Four phases, sequenced by what each needs from the one before.

**Phase 1 — the boundary.** Add `Boundary`, `entry_slice`, `epoch_slice` and
`delivery_window`; point `latest_epoch_gate_failed` at `epoch_slice`; rename the
delivery check and point it at `delivery_window`. Invert the two unit tests that
assert a self-entry resets delivery, and add the ones the PRD's criteria name: a
self-entry with no record anywhere, a same-phase rewind, and the R15 case where
one synthetic log gives the gate classification and the delivery check opposite
answers. Rewrite the doc comments on all three functions, including the one that
argues the two must not disagree.

Observable behavior changes at the end of this phase, because the delivery check
is already wired.

**Phase 2 — the call sites.** Update both `koto next` construction sites for the
rename, and replace the directed path's "provably false" proof with the argument
that now holds. One existing test outside the delivery file has a setup that
becomes a suppressing arrival under the new rule; the PRD names it and prescribes
the remedy.

**Phase 3 — behavioral coverage.** Add the integration cases the PRD's criteria
enumerate. One of them cannot be expressed against the existing template constant
and needs a new one, which is the only part of this phase the criteria do not
already settle. Verify the byte-identity fixture is untouched except for the one
prose string, edited in lockstep with its source.

**Phase 4 — the record.** Rewrite the nine agent-facing and operator-facing
surfaces, including the eval suite's explanations of the two evals R20 protects
and the narrating comments in the delivery test; correct the two "not emitted
yet" claims; amend `PRD-inline-phase-details.md` and
`DESIGN-inline-phase-details.md` in place, rewriting the contradiction passage
into a record of both rulings; add one cross-reference to
`DESIGN-koto-next-output-contract.md`'s superseded mechanism decision; add the
new eval; write the changelog entry, which also carries a now-dangling reference
to the renamed function.

Phases 1 and 2 could be one commit and phases 3 and 4 could each be several. The
sequence matters more than the granularity: the boundary lands before the call
sites depend on its name, and the record lands only once the behavior it
describes is real.

## Security Considerations

No new attack surface. The change adds no input parsing, no new file access, no
new command execution, no network path, and no privilege boundary. It reads
events the tick has already loaded and decides one field of one JSON response.

Three things were considered and found not to apply:

**Log tampering.** Session state files are created `0600`, which is what gates
write access on a shared host. An attacker who can write one can already append a
transition to any phase, choose what evidence it holds, and decide which gates
passed. Being able to also suppress a `details` field adds nothing to that.

**Denial by starvation.** The change makes it possible for an agent inside a loop
never to receive a phase's instructions again. That is the intended behavior, and
it is bounded by two things the design keeps: `koto status` returns them without
moving the workflow, and every non-error response for an instruction-carrying
phase carries a pointer naming that command. The error variant is the exception,
and it is pre-existing: a tick that errors carries neither the instructions nor
the pointer, and recovers on the next non-error tick. An implementation that
re-gated the pointer on whether the response carries the instructions — a
one-token edit at either splice, which nothing type-checks against — would turn a
saving into a trap. That is why the pointer has its own requirement and its own
acceptance criteria, and why the test covering it should be read as structural
rather than incidental.

**Unbounded scan.** The delivery window can span an arbitrarily long loop. The
check scans forward and short-circuits on the delivery record, which the same
tick that opened the window appended a handful of events later, so a long loop
does not make the scan long. The worst case is a window holding no record at all:
one forward pass over data already in memory, and at most once, because a scan
that finds nothing delivers and records, so the next tick short-circuits again.

**Concurrent writers.** koto#200 is an open defect: the plain append path
computes its sequence number and then writes without a lock, so two writers can
claim the same one. The interaction with this change is favorable rather than
adverse. A log with a sequence gap is rejected wholesale by the reader rather
than silently mis-answered, so the case worth worrying about — a lost arrival
event that would reopen the window at an earlier visit — cannot produce a wrong
delivery answer, only a refused read, and it refuses identically under both
rules. A torn final line drops one event; if that is the delivery record the next
tick re-delivers, which is the safe direction and is also unchanged. Meanwhile a
suppressing lap appends one event where today it appends two, which shrinks
rather than widens the window in which two writers can collide.

The change does not touch `CURRENT_SCHEMA_VERSION`, the frozen `SessionBackend`
surface, or `koto::engine::types`, so nothing in `docs/STABILITY.md` moves.

## Consequences

### Positive

- A loop pays for its phase's procedure once. On the fourteen-week sweep that
  prompted koto#90's audit, that is one delivery instead of fourteen.
- The code stops using one word for two things. `epoch_slice` and
  `delivery_window` say which question they answer, and the unit test for R15
  reads as one log through two functions with opposite answers.
- The rewind answer is structurally independent of koto#199. Whatever that fix
  changes about which events a rewind appends, the rewind arm does not read them.
- Two comments that were true when written and false when merged get corrected:
  the directed path's proof, and the claim that the delivery event is not emitted.
- The durable record carries both rulings on the question and says which governs,
  so the next reader inherits the argument rather than re-deriving the discarded
  half of it.

### Negative

- The failure direction inverts for one case. Inside a loop that has already
  delivered, an agent that loses the procedure will not get it back by ticking.
  Mitigated by `koto status` and the pointer, both of which become load-bearing
  in a way they were not before — which is why they carry requirements and
  criteria rather than sitting in the background.
- An in-flight session sitting on a self-entry when the new binary lands
  suppresses where the old one delivered, on its very next tick. No migration
  avoids this without keeping the old rule for old logs, which would mean two
  rules.
- The rename touches a function the gate path depends on, so a reviewer has to
  satisfy themselves that `epoch_slice` is the old behavior. It is, by
  construction of the `AnyEntry` arm, but it is work the smallest possible diff
  would not have asked for.
- The file still carries four backwards scans over entry events, three of them
  open-coded. This change does not reduce that and is not the place to.
- The byte-identity fixture takes a diff, in the one file whose purpose is not to
  have one. Bounded by an acceptance criterion that permits exactly the one prose
  string and forbids any change inside a recorded response body.

### Mitigations

- The R15 unit case is the guard against the whole class of "we moved more than
  we meant to": it fails loudly on an in-place widening of the shared scan, which
  is the implementation a future maintainer is most likely to reach for.
- Every adjacent behavior that must not move is pinned by an existing test the
  PR is required to leave with its assertions unmodified, and the criteria name
  them individually so the test diff is fully predicted by the documents.
- The change is demonstrated against a built binary across thirteen cases, with
  the measured output recorded in the pull request, so a reviewer can check the
  behavior without re-running it.
