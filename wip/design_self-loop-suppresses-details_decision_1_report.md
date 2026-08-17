# Decision 1: where the delivery-window boundary lives

**Authorship note.** Two decision-researcher agents were dispatched for this
question and both died on transient API errors without writing anything. Rather
than block the chain on a third attempt, the design author performed the analysis
directly against the source. Every file:line citation below was read first-hand,
and the two empirical facts the decision turns on were measured with a built
binary rather than inferred.

## Question

The delivery decision and the gate-blocked classification currently read one
boundary. PRD R15 requires them to stop agreeing: the delivery window must skip a
self-entry while the gate epoch must keep closing at one. Where does the new
boundary live, and how is the rewind exemption expressed so that the answer
cannot depend on koto#199?

## What is there today

`occupancy_slice` (`src/engine/persistence.rs:1028`) is a private backwards scan.
It finds the last event whose payload is a `Transitioned`, `DirectedTransition`
or `Rewound` with `to == current_state`, and returns everything after it, or the
whole log when no such event exists.

Two consumers:

- `latest_epoch_gate_failed` (`:1058`), public, read by the dashboard
  (`src/cli/dashboard_data.rs:458`) and the `/workflows` projection writer
  (`src/workflows_surface/project.rs:183`).
- `instructions_delivered_this_occupancy` (`:1099`), public, read by both
  `koto next` response-construction sites through one combinator
  (`src/cli/mod.rs:3417` and `:4298`).

Its doc comment argues the sharing is a correctness property: "Shared rather than
copied so the predicates built on it -- the epoch-scoped gate classification and
the delivery check -- cannot come to disagree about where an occupancy starts"
(`:1017-1019`). That sentence is what this decision has to overturn deliberately
rather than quietly.

Three further backwards scans in the same file open-code the same walk without
calling the helper: `derive_evidence` (`:722`), `derive_overrides` (`:796`), and
`derive_last_gate_evaluated` (`:844`). So the file already contains four
implementations of "since this phase was last entered", and they agree today by
coincidence rather than by construction. That is context for the diff-size
argument below: no option here reduces that number, and one option raises it.

The two facts the options are evaluated against, both measured against a built
binary on merged `main`:

- `koto rewind` issued immediately after a self-transition appends
  `Rewound { from: "implement", to: "implement" }`. A same-phase rewind is
  reachable from the CLI, not merely constructible in a unit test.
- A `P -> Q -> P` round trip inside a single tick appends a final
  `Transitioned { from: "Q", to: "P" }`. The tick begins and ends in the same
  phase and is nonetheless a genuine arrival.

## Options considered

### Option 1: a sibling scan

A new private `delivery_window(events, state)` duplicating the walk with one
extra condition; `occupancy_slice` untouched.

**Consequences.** Smallest conceptual change and zero risk to the gate epoch: the
function backing it is not edited at all. The `git diff` criterion R15 names
passes trivially.

**How it fails.** The file goes from four near-identical backwards scans to five,
and the two that matter most sit adjacent with almost the same body and a
one-line difference. The next person to fix a bug in one will not find the other.
The shared-helper doc comment also becomes false without being edited, which is
the worst state for a comment to be in — it still asserts a property the code no
longer has, in a file where somebody just demonstrated that such comments get
believed.

### Option 2: a boundary-policy parameter

One scan, `entry_slice(events, state, boundary)`, with the two existing consumers
passing different values.

**Consequences.** One scan implementation, so the two boundaries cannot drift
apart in their handling of anything except the boundary itself. The R15 unit case
reads as one log through one function with two arguments.

**How it fails.** The discriminator lands at the call site. `latest_epoch_gate_failed`
would read `entry_slice(events, state, Boundary::AnyEntry)` and the delivery
check `entry_slice(events, state, Boundary::ArrivalFromElsewhere)`, and a
mistaken argument at either site is a silent behavior change with no type error.
It also leaves the shared-helper comment self-contradictory: shared so they
cannot disagree, parameterized so they do.

### Option 3: rename and split, over one shared inner scan

Rename `occupancy_slice` to a name that says which boundary it computes, add a
second named function for the delivery window, and implement both over one
private inner scan parameterized by boundary. The parameter exists but never
appears at a call site.

Sketch:

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

`latest_epoch_gate_failed` calls `epoch_slice`; the delivery check calls
`delivery_window`. Neither names a boundary value.

**Consequences.** One scan, so no fifth duplicate and no drift in anything but
the boundary. Two names, so the R15 unit case is written as one log through two
functions with opposite answers — which is exactly what the requirement says and
exactly what a reader will check. The rename also moves the code onto the PRD's
vocabulary: "occupancy" now means two different things depending on which
document you open, and retiring it from the code is most of what stops the next
reader repeating this confusion. The doc comment gets rewritten to say why the
two boundaries legitimately differ, which R19's spirit asks for in the code as
well as in the documents.

**How it fails.** It is the largest diff of the three, and the rename touches a
function the gate path depends on, so a reviewer has to satisfy themselves that
`epoch_slice` is byte-for-byte the old behavior. That is cheap to check —
`Boundary::AnyEntry` reproduces the old match arm by construction — but it is
work the other two options do not ask for.

### Option 4 (considered, rejected): widen `occupancy_slice` in place

Edit the shared helper so it skips self-entries, and let both consumers follow.

Rejected on measurement, not taste. `latest_epoch_gate_failed` takes the *latest*
gate evaluation inside its slice, so widening only changes its answer when the
narrow slice held no gate evaluation and the wide one does. That is precisely the
tick right after a self-entry: today the stale verdict drops out and the badge
clears; widened, the pre-loop failed gate stays in scope and the badge sticks.
`latest_epoch_gate_failed` has no direct test coverage anywhere in the repo, so
this would land silently. It is the outcome PRD R15 exists to forbid, and the
R15 unit criterion is written to fail loudly against exactly this option.

## Recommendation

**Option 3.** One scan, two names, no boundary argument at any call site.

Option 1 was right that the gate epoch must not be touched, and Option 3 honours
that by keeping `epoch_slice` behaviorally identical. Option 2 was right that
duplicating the scan is the wrong price to pay in a file that already carries
three open-coded copies, and Option 3 keeps its single implementation while
denying it the call-site footgun. Option 4 was the cheapest thing that could
possibly work and is the one the requirement was written against.

The rename is the part that earns its cost. The failure this whole change exists
to correct is that a definition got written down, believed, and never
re-examined. Leaving the code calling the gate boundary an "occupancy" while
three documents redefine that word is the same shape of error one size smaller.

## The rewind exemption

Expressed in the match arm, by not binding `from` on the rewind variant at all:

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

The disqualified alternative is testing `from != to` uniformly across all three
variants and then carving `Rewound` back out with a second condition. It reaches
the same answers today, and it is one deleted line away from coupling the
delivery rule to rewind's destination-selection logic — the exact coupling R6
exists to prevent, in the exact file a koto#199 fix will be editing. Not binding
the field is a structural guarantee rather than a remembered one.

**`from: None`.** Initial entry records no source, and
`from.as_deref() != Some(state)` is true for `None`, so initialization opens a
delivery window and the first tick delivers. R2 holds without a special case. A
legacy log whose `from` is absent deserializes to `None` and reads the same way,
which is the safe direction: a missing field produces a delivery rather than a
silence.

## Open questions for the design author

None blocking. Two notes to carry into the design:

- The long comment on the directed path (`src/cli/mod.rs:3377-3402`) argues that
  the delivery check "provably evaluates to `false` on every call" there. Under
  this decision that stops being true for `koto next --to P` issued at `P`: the
  synthetic `DirectedTransition { from: P, to: P }` is not a window opener, so
  the scan reaches back to the real arrival and finds its delivery record. The
  comment has to be replaced with the new argument, not trimmed.
- `derive_evidence`, `derive_overrides` and `derive_last_gate_evaluated` keep
  their open-coded scans. That is deliberate: folding them into `epoch_slice`
  would be a behavior-preserving refactor with its own blast radius, and doing it
  in the same change as a behavior reversal would make the diff unreviewable.

## Summary

The shipped boundary helper is shared between the delivery check and the gate
classification, and PRD R15 now requires them to differ, so the decision is how
to make them differ without duplicating a scan the file already carries three
open-coded copies of. The recommendation is one private scan parameterized by
boundary, exposed through two named wrappers — `epoch_slice` preserving today's
behavior for the gate path and `delivery_window` skipping self-entries — so no
call site ever passes a boundary and the R15 unit case reads as one log through
two functions with opposite answers. The rewind exemption is expressed by not
binding `from` on the rewind arm at all, which makes the independence from
koto#199 structural rather than remembered.
