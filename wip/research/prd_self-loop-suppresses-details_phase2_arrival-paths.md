# Lead: every arrival path and which are self-entries

Read-only survey of `tsukumogami/koto` @ `docs/self-loop-suppresses-details`
(HEAD `e79531f`). No koto session was created, no state mutated, no build run.

## Findings

### The complete set of state-entry construction sites

A phase becomes "the phase the response is built for" only by being the target
of a **state-entry event** — `Transitioned`, `DirectedTransition`, or `Rewound`.
Those three are the only variants `derive_state_from_log`
(`src/engine/persistence.rs:708-715`) and `occupancy_slice`
(`src/engine/persistence.rs:1028-1046`) look at.

There are exactly **seven** places in production code that construct one. Every
other `EventPayload::{Transitioned,DirectedTransition,Rewound}` literal in the
tree is inside a `#[cfg(test)]` module (`src/engine/persistence.rs` from 1109,
`src/engine/types.rs` from ~2050, `src/cli/overrides.rs` from 377,
`src/workflows_surface/project.rs` from 377, `src/workflows_surface/materialize.rs`
from 330, `src/session/local.rs` from 776, `src/cli/batch.rs` from 3031,
`src/cli/dashboard_data.rs`). I checked each against its module's `#[cfg(test)]`
line number; the seven below are the only live ones.

| # | Path | Trigger | Event appended (`from` → `to`) | Same phase possible? | Site |
|---|------|---------|-------------------------------|----------------------|------|
| 1 | Initial entry, file template | `koto init`, `koto init --parent`, batch child spawn, skip-marker spawn | `Transitioned { from: None, to: initial_state, condition_type: "auto" }` | N/A (no predecessor) | `src/cli/init_child.rs:502-507` |
| 2 | Initial entry, inline template | `koto init --from-stdin`, `init_inline_into_session` | `Transitioned { from: None, to: initial_state, condition_type: "auto" }` | N/A | `src/cli/init_child.rs:671-676` |
| 3 | `skip_if`-driven transition | advance loop step 7, `skip_if` conditions all satisfied | `Transitioned { from: Some(state), to: target, condition_type: "skip_if", skip_if_matched: Some(map) }` | **Yes**, first hop only | `src/engine/advance.rs:503-508` |
| 4 | Conditional **and** unconditional transition | advance loop step 8, `resolve_transition` → `Resolved` | `Transitioned { from: Some(state), to: target, condition_type: "auto", skip_if_matched: None }` | **Yes**, first hop only | `src/engine/advance.rs:545-550` |
| 5 | Directed transition | `koto next --to <phase>` | `DirectedTransition { from: current, to: target, rationale }` | **Yes**, unrestricted | `src/cli/mod.rs:3336-3340` |
| 6 | Operator rewind | `koto rewind` | `Rewound { from: current, to: prev_entry.to, rationale }` | **Yes** | `src/cli/mod.rs:2044-2048` |
| 7 | Batch retry rewind (child log) | `koto next` on a parent with a `retry_failed` submission | `Rewound { from: child_current, to: child_initial_state }` | **Yes** | `src/cli/retry.rs:551-555` |

Note that paths 3 and 4 are one construction site each but **three** of the
lead's enumerated classes: conditional, unconditional, and `skip_if` all funnel
through `resolve_transition` and differ only in `condition_type` /
`skip_if_matched`. There is no separate unconditional site — an unconditional
transition is the `unconditional_target` fallback branch inside
`resolve_transition` (`src/engine/advance.rs:757-769`) and emits the same
`condition_type: "auto"` payload as a conditional match.

### Per-path detail

#### 1 & 2 — `koto init` (arrival at the initial state)

`handle_init` (`src/cli/mod.rs:1720`) delegates to `init_child_from_parent` →
`init_child_core` (`src/cli/init_child.rs:403`), which writes a two-event log:
`WorkflowInitialized` then

```rust
    let transition_payload = EventPayload::Transitioned {
        from: None,
        to: initial_state.clone(),
        condition_type: "auto".to_string(),
        skip_if_matched: None,
    };
```

`src/cli/init_child.rs:502-507`. `initial_state` is the template's
`initial_state` unless `override_initial_state` is `Some` (the skip-marker spawn
path, `src/cli/init_child.rs:398-401,442-445`). The inline path
(`init_inline_into_session`, `src/cli/init_child.rs:584`) writes the identical
shape at `src/cli/init_child.rs:671-676`.

`koto init` itself prints nothing but `{name, state}` — the first `koto next`
after it is what builds a response for the initial phase. At that point
`occupancy_slice` finds the init `Transitioned` as the entry event, the slice
after it holds no `InstructionsDelivered`, and the phase delivers.

Self-entry: not applicable. There is no predecessor phase and `from` is `None`.

`koto session start` (`src/cli/session.rs:354-373`) writes **only**
`WorkflowInitialized` and no `Transitioned` at all — those sessions have no
template and never reach `handle_next`'s response builder.

#### 3 — `skip_if`-driven transition

`src/engine/advance.rs:478-524`. When `conditions_satisfied` returns true, the
loop calls `resolve_transition` with the assembled evidence, and on `Resolved`
appends the payload above with `condition_type: "skip_if"`. Cycle detection runs
**before** the append (`src/engine/advance.rs:494-501`).

#### 4 — conditional / unconditional transition

`src/engine/advance.rs:526-560`. `resolve_transition`
(`src/engine/advance.rs:693-779`) returns `Resolved` for either exactly one
matching `when` clause, or (no conditional match) the unconditional fallback —
but the fallback is withheld and turned into `NeedsEvidence` when `gate_failed`,
or when `!fresh_evidence && has_conditional` (`src/engine/advance.rs:757-769`).
`fresh_evidence` starts as `!evidence.is_empty()` and is set `false` after every
auto-advance (`src/engine/advance.rs:198,515,559`).

That flag is what bounds multi-hop chains: a state carrying both conditional and
unconditional transitions will not auto-advance through on a hop it was chained
into. A pure-routing state (unconditional only, `has_conditional == false`) does
chain through.

#### 5 — multi-hop auto-advance in one tick, including return to the start

The `visited` set is deliberately **not** seeded with the starting state:

```rust
    // The starting state is NOT added to visited. The visited set tracks states
    // we've auto-advanced THROUGH during this invocation. The starting state was
    // already arrived at before this invocation, so re-visiting it (e.g., in a
    // review -> implement -> review loop) is legitimate.
```

`src/engine/advance.rs:210-213`. So a tick starting at `P` can run
`P → Q → P` and stop there: the hop into `Q` inserts `Q`, the hop back into `P`
finds `P` not in `visited` and is allowed, and the next attempted hop to `Q`
trips `CycleDetected`. The existing unit test at `src/engine/advance.rs:2426-2434`
asserts exactly this shape (`final_state: "a"`, `advanced: true`,
`CycleDetected { state: "b" }`).

**This is the case the PRD most needs to name, because it is a tick that starts
and ends in `P` but is not a self-entry.** The last entry event is
`Transitioned { from: Some("Q"), to: "P" }`. The agent did leave the phase, ran
`Q`, and came back. Under the BRIEF's outcome ("arriving at a phase from
somewhere else — including coming back to one it visited earlier — still
delivers") this must deliver. Today it does deliver, because `occupancy_slice`
starts at that entry event and finds no `InstructionsDelivered` after it. A rule
phrased as "if the tick's start phase equals the tick's end phase, suppress"
would get this wrong.

Intermediate phases the loop passes through get no response built for them at
all — the response is built once, for `final_state` — so they never deliver and
never record an `InstructionsDelivered`. The BRIEF puts that question explicitly
out of scope.

#### 6 — self-transition `P -> P` reached by the advance loop, and its bound

Cycle detection is `visited.contains(&target)` checked before the append
(`src/engine/advance.rs:495-501` for `skip_if`, `536-542` for the ordinary path),
and `visited.insert(target)` happens immediately after each append
(`src/engine/advance.rs:510,553`). Combined with the empty-at-entry `visited`
set, this yields a precise rule:

- **A self-transition is emitted only as the first hop of a tick, always from
  the tick's starting phase.** On hop 1 `visited` is empty, so `target == state`
  passes the check.
- **Every later self-transition is refused.** Any state reached after hop 1 was
  inserted into `visited` at the moment it was entered, so a transition targeting
  itself always finds itself in `visited` and returns
  `StopReason::CycleDetected { state: P }` with no event appended.

So at most one `Transitioned { from: Some(P), to: P }` per `koto next`
invocation. After it fires, the loop re-enters at `P` with
`current_evidence = BTreeMap::new()` and `fresh_evidence = false`, which for the
usual conditional self-loop means `resolve_transition` returns `NeedsEvidence`
and the tick stops at `P` with `advanced: true`. A *pure-routing* self-loop
(`P` with a single unconditional transition to `P`, no conditional siblings) does
re-resolve, and is caught by cycle detection on hop 2 —
`CycleDetected { state: P }`, `advanced: true`, one event written. It cannot
spin.

A self-transition can also be followed by hops to *other* phases in the same
tick (`P → P → Q`), if `P`'s post-loop resolution still fires. The tick then ends
at `Q`, an ordinary cross-phase arrival.

#### 7 — `koto next --to <phase>` (directed transition)

`src/cli/mod.rs:3286-3353`. Validation, in order:

1. The current state must exist in the template (`3291-3302`, else `TemplateError`).
2. **The target must be a declared transition target of the current state**
   (`3305-3322`): `valid_targets` is `current_template_state.transitions.iter().map(|t| t.target)`,
   and a miss is `PreconditionFailed` with `"state '<cur>' does not have a
   transition to '<target>'"`. Note this reads the *target list only* — `when`
   clauses are ignored, so `--to` can take a conditional edge without the
   evidence that edge requires.
3. The target must exist in the template (`3325-3333`, else `TemplateError`).

Consequence for `--to P` while already at `P`: **it is accepted exactly when the
template declares a `P -> P` transition**, and rejected with `PreconditionFailed`
otherwise. A template with no self-loop cannot produce a directed self-entry at
all. The integration test `two_consecutive_directed_transitions_into_same_phase_both_carry`
(`tests/instructions_delivery_test.rs:486-513`) exercises this and asserts today's
(to-be-reversed) behavior: both directed arrivals carry details.

The directed path skips the advance loop entirely (single-shot), skips gate
evaluation (`gate_results` is an empty map, `src/cli/mod.rs:3357`), and builds
its `already_delivered` verdict from an **in-memory** event list — the pre-tick
`events` plus a synthetic `Event` wrapping the payload just appended
(`src/cli/mod.rs:3403-3418`) — rather than re-reading the log. The comment at
`3382-3402` records that this provably evaluates to `false` on every call today,
because the synthetic entry event is always the newest element and the occupancy
slice after it is always empty. **Any suppression rule for directed self-entries
has to change something other than this call site's inputs** — feeding the same
predicate the same list will keep answering "not delivered".

#### 8 — `koto rewind`

`handle_rewind`, `src/cli/mod.rs:1985-2089`. Destination selection:

```rust
    let state_changing: Vec<&Event> = events.iter()
        .filter(|e| matches!(e.payload,
            EventPayload::Transitioned { .. }
                | EventPayload::DirectedTransition { .. }
                | EventPayload::Rewound { .. }))
        .collect();
    if state_changing.len() <= 1 { /* "already at initial state, cannot rewind" */ }
    let current_state = derive_state_from_log(&events).unwrap_or_default();
    let prev_event = state_changing[state_changing.len() - 2];
    let prev_state = /* prev_event's `to` */;
```

`src/cli/mod.rs:2012-2038`. The destination is **the `to` of the second-to-last
state-entry event**, not a computed predecessor. Two consequences:

- `Rewound { from: P, to: P }` **is reachable**, and easily. Any log whose last
  two state-entry events both name `P` produces it: a self-transition
  (`… T{to:P}, T{to:P}` → rewind gives `from: P, to: P`), or a directed `--to P`
  issued while at `P`. This is precisely the case the BRIEF says must still
  deliver ("including a rewind that lands on the phase it started from").
- Because `Rewound` is itself in the `state_changing` filter, consecutive rewinds
  oscillate between two phases. That is the separate filed defect the BRIEF lists
  as adjacent-but-out-of-scope; it matters here only because it means a rewind
  destination is *not* reliably "the phase before this one".

Rewind also relocates children to an epoch branch when the `from` state carries a
`materialize_children` hook (`rewind_relocate_children`, `src/cli/mod.rs:2096`),
and prints `{name, state, children, superseded_branch, children_relocated}` —
but it does **not** build a `koto next` response. The delivery decision for a
rewind lands on the *next* `koto next` tick, which sees `Rewound` as the entry
event in `occupancy_slice`.

#### 9 — batch / child spawning and `materialize_children`

The scheduler spawns children through `init_child_from_parent`
(`src/cli/batch.rs:1569`) and `init_child_as_skip_marker_from_parent`
(`src/cli/batch.rs:1655`), i.e. path 1 above, writing the initial entry event
into each **child's** log. Nothing is appended to the parent's log except
`SchedulerRan` (`src/cli/mod.rs:4392`) and, on finalization, `BatchFinalized`
(`src/cli/mod.rs:4449`) — neither is a state-entry event. **Spawning does not
move the parent's phase.** The parent's response that tick is whatever the
advance loop produced for the parent.

`src/workflows_surface/materialize.rs` is unrelated to children despite the name:
it is the `/workflows` artifact renderer called off the commit funnel
(`materialize_after_commit`, `src/workflows_surface/materialize.rs:46`). It
appends nothing; its only `EventPayload::Transitioned` literal
(`materialize.rs:366`) is a test helper.

#### 10 — gate overrides

`koto overrides record` appends `GateOverrideRecorded` and nothing else
(`src/cli/overrides.rs:278-286`). **No transition.** The next `koto next` reads
the override via `derive_overrides` and injects a synthetic `Passed` result
(`src/engine/advance.rs:330-369`), which may *unblock* an advance that then
transitions through path 3/4 — but the override itself is not an arrival. Both
`EventPayload::Transitioned` literals in `overrides.rs` (489, 525) are inside its
`#[cfg(test)]` module (starts at line 377).

#### 11 — retry paths

`src/cli/retry.rs`. Three shapes, all operating on **child** sessions:

- Failed real-template child → `write_rewound_event`
  (`src/cli/retry.rs:533-558`): `Rewound { from: child_current, to: child_initial_state }`.
  **Self-entry when the child failed at its own initial state**, and the
  documented fallback (`src/cli/retry.rs:538-546`) deliberately produces
  `from == to` ("vacuous rewind") when the template can't be read.
- Skipped child → `respawn_skipped_child` (`src/cli/retry.rs:571`) — session is
  deleted and re-spawned, so the new log's first entry event is path 1.
- Spawn-failed child → `respawn_failed_child` — same, path 1.

On the **parent**, retry appends only `EvidenceSubmitted` payloads
(`src/cli/retry.rs:410,428`) and splices `retry_dispatched` onto the envelope
(`src/cli/mod.rs:4566-4578`). No parent transition.

#### 12 — non-advancing ticks

Gate blocked (`advance.rs:431-435`, `573-577`), evidence required
(`563-569`), unresolvable (`579-583`), terminal (`245-249`), integration
(`256-263`), integration unavailable (`266-282`), action-requires-confirmation
(`302-311`), signal received (`218-222`), chain limit (`227-231`), cycle detected
(`496-500`, `537-541`) — **none of these append a state-entry event.** Gate
evaluation does append `GateEvaluated` (`advance.rs:382-390`), and a
confirmation-requiring action appends `DefaultActionExecuted`
(`src/cli/mod.rs:4025`), but the phase does not change, so `occupancy_slice`
still starts at the previous entry event and `already_delivered` is `true` for a
phase that already delivered in this occupancy. Non-advancing ticks therefore
suppress today, correctly, and this work must not disturb that.

`Terminal` and `Error` responses never carry `details` at all
(`NextResponse::carries_details`, `src/cli/next_types.rs:492-501`;
`with_details_suppressed_unless_full` returns both unchanged, `next_types.rs:479-480`),
so a terminal phase's instructions are only reachable via `koto status`… and
`handle_status` also withholds them when the phase is terminal
(`src/cli/mod.rs:5081-5082`). Pre-existing, out of scope, noted because it is the
one phase class with no delivery path.

#### 13 — `koto status`

`handle_status`, `src/cli/mod.rs:4977-5140`. Confirmed: **appends nothing and
takes no lock.** Every operation is a read — `backend.exists`,
`backend.read_events`, `derive_machine_state`, `std::fs::read` of the template,
`sha256_hex` — and the source says so at `src/cli/mod.rs:5077-5080`:

```
    // This retrieval always returns the full instructions regardless of
    // the delivery rule `koto next` applies (PRD R10), and it appends
    // nothing: no delivery record, no other event, and no lock is taken
    // anywhere in this function.
```

There is no `append_event` call anywhere in the function; the only
`append_event` sites in `mod.rs` are at 2050 (rewind), 2294, 2423, 2859, 3345
(directed), 3462 + 4607 (`InstructionsDelivered`), 3821, 3909, 4032, 4402, 4455,
4870 — all outside `handle_status`'s range.

### Is `Transitioned.from` ever `None` on a genuine phase-to-phase move?

**No.** `from: Option<String>` (`src/engine/types.rs:469-471`) is `None` at
exactly two construction sites, both initial-entry:

- `src/cli/init_child.rs:503` — file-template init
- `src/cli/init_child.rs:672` — inline init

Every advance-loop construction sets `from: Some(state.clone())`
(`src/engine/advance.rs:504`, `546`). `DirectedTransition.from` and
`Rewound.from` are non-optional `String`s (`types.rs:507`, `514`), so they cannot
be absent at all.

Two caveats worth carrying into the PRD:

1. **`condition_type` does not distinguish initial entry.** Both init sites write
   `condition_type: "auto"`, the same value the ordinary advance-loop transition
   writes. Only `from: None` marks an initial entry. (A `"initial"` string appears
   at `src/session/local.rs:1428` but that is a test fixture, and the
   deserializer accepts any string — `types.rs:1404-1410`.)
2. **`from` is untrusted at read time.** `TransitionedPayload.from` is plain
   `Option<String>` with no validation on deserialize, and nothing today reads
   `from` for any behavioral decision — the delivery predicate, state derivation,
   and evidence derivation all key on `to` and position only
   (`persistence.rs:708-715`, `727-745`, `1028-1046`). A rule that starts reading
   `from` starts trusting a field that has never been load-bearing and that a
   hand-edited or externally-produced log can set freely.

## Implications

**The PRD needs a requirement per arrival class, and the classes are these
five**, because they are what the seven construction sites collapse into once you
ask "same phase or not":

1. **Initial entry** (`from: None`, both init sites). Always delivers. Never a
   self-entry — there is no predecessor. A requirement here is about not
   regressing the first tick after `koto init`.
2. **Cross-phase arrival** (advance loop with `from != to`; directed `--to Q`
   from `P`; the `Q → P` tail of a multi-hop chain, including one that started at
   `P`). Always delivers, including re-entry into a phase visited earlier.
3. **Self-entry via the advance loop** (`Transitioned { from: Some(P), to: P }`).
   Suppresses. Bounded at one per tick and only as the tick's first hop.
4. **Self-entry via a directed transition** (`DirectedTransition { from: P, to: P }`,
   possible only when the template declares `P -> P`). Suppresses.
5. **Any rewind** (`Rewound`, whether `from != to` or `from == to`, whether from
   `koto rewind` or from batch retry on a child). Always delivers.

Plus the two non-arrival requirements the BRIEF already fixes: `koto next --full`
always delivers regardless of class, and `koto status` always returns the
instructions and appends nothing.

**The PRD must state the rule in terms of the last state-entry event, not in
terms of the tick.** "The tick started and ended in `P`" is not the same
proposition as "the entry event that opened `P`'s current occupancy names `P` as
its source". The `P → Q → P` chain satisfies the first and not the second, and
the BRIEF's outcome requires it to deliver. Symmetrically, "`P` was already the
current state when the command ran" is true for both a directed self-entry
(suppress) and a rewind that lands back on `P` (deliver), so that phrasing does
not discriminate either. The rule that discriminates all five classes correctly
is: **suppress iff the entry event opening the current occupancy is a
`Transitioned` or `DirectedTransition` whose `from` equals its `to`.**

**Gate overrides, batch spawning, `SchedulerRan`/`BatchFinalized`, retry on the
parent, context add/remove, decisions, evidence submission, respawn and wake all
append non-entry events.** They are not arrival classes and need no requirement —
but the PRD should say so explicitly, because "does an override re-deliver?" is a
question a reader will ask and the answer ("no, nothing moved") is only obvious
once you know the event taxonomy.

**Non-advancing ticks already suppress and must be shown not to have moved.** The
BRIEF puts "adjacent behaviors that must be shown not to have moved" in scope;
the gate-blocked / evidence-required / cycle-detected / confirm /
integration-unavailable ticks are the concrete list, and they suppress today for
a *different* reason (no new entry event) than the one this work introduces.

**One thing the BRIEF's outcome does not cover, and the PRD should:** the
delivery predicate shares `occupancy_slice` with `latest_epoch_gate_failed`
(`persistence.rs:1058`) — the dashboard's and `/workflows`' "blocked"
classification — and the same boundary is re-implemented in `derive_evidence`
(`persistence.rs:717-745`). All three treat a self-transition as an epoch
boundary. Only the *delivery* boundary is meant to move. If the PRD phrases its
requirement as "change what an occupancy is", it silently retargets the gate
classification and risks retargeting the evidence epoch, and the evidence epoch
resetting on a self-transition is what makes a retry loop work at all. The PRD
should scope the requirement to the delivery decision and say the epoch boundary
used for evidence and gate classification is unchanged.

**A second uncovered point:** the directed-transition call site
(`src/cli/mod.rs:3403-3418`) is documented as provably always answering "not
delivered", with a long comment defending that as intentional rather than dead
code. Requirement 4 above reverses its answer for the `from == to` case. The PRD
should state the outcome (a directed transition into the already-occupied phase
suppresses) without prescribing that the shared predicate be the thing that
changes — the two call sites feed it different inputs (one an in-memory list, one
a fresh read), and that asymmetry is a design concern.

**Third:** a terminal phase never delivers `details` on any path, `koto status`
included. Out of scope, but the PRD's "every arrival delivers except self-entries"
phrasing would be literally false without a carve-out sentence.

## Surprises

- **`materialize_children` is not `src/workflows_surface/materialize.rs`.** That
  file is the `/workflows` JSON artifact renderer and appends nothing; the
  children hook is handled in `src/cli/batch.rs` via `init_child_from_parent`.
  Easy to mis-cite in a PRD.
- **A self-transition can only ever be the first hop of a tick.** The `visited`
  set makes every later self-transition a `CycleDetected` stop with no event
  written. So "how many self-entries can one `koto next` produce?" has the answer
  "at most one", which bounds the requirement neatly and was not obvious.
- **`P → Q → P` in a single tick is a supported, tested shape**
  (`src/engine/advance.rs:2426-2434`), and it is a tick that begins and ends in
  the same phase while being a genuine cross-phase arrival. This is the single
  most likely way to get the rule wrong.
- **`koto next --to P` at `P` is only possible when the template declares
  `P -> P`.** The validator checks the target list, so a template with no
  self-loop cannot produce a directed self-entry — the requirement is
  conditional on template shape.
- **Rewind's destination is the second-to-last entry event's `to`, verbatim.**
  Not a computed predecessor. That is why `Rewound { from: P, to: P }` falls out
  naturally after a self-transition, and why consecutive rewinds oscillate.
- **`koto retry`'s fallback deliberately writes a `from == to` rewind** and calls
  it a "vacuous rewind" in the comment (`src/cli/retry.rs:539-546`) — a
  self-rewind that already exists in the code by design.
- **`condition_type` is `"auto"` at init**, identical to an ordinary transition.
  The only initial-entry marker in the log is `from: None`.

## Open Questions

1. Should the delivery rule read `Transitioned.from`, a field that has never been
   load-bearing and is unvalidated on deserialize, or should it compare the last
   two entry events' `to` values (which needs no new trust but misreads a log
   whose entry events were interleaved by a concurrent writer)? A PRD-level
   decision only insofar as it constrains what the requirement can be stated over;
   otherwise the DESIGN's.
2. Legacy logs written before this change carry self-transitions that already
   delivered. On the first tick after upgrading, a session sitting on a
   self-entry will suppress where the old binary delivered. Is that acceptable,
   or does the PRD need a statement about in-flight sessions?
3. `koto retry`'s `Rewound { from: P, to: P }` lands on a **child** session. The
   BRIEF's "any rewind delivers" reads as covering it, but the journey it is
   written for is an operator rewinding a phase. Confirm the retry rewind is in
   scope rather than incidentally swept in.
4. `P → P → Q` in one tick: the self-entry is recorded but no response is ever
   built for it (the response is built for `Q`). Does the requirement need to say
   anything about a self-entry the agent never observes, or is it silently
   correct?

## Summary

Seven production sites construct a state-entry event, and they collapse into five
arrival classes the PRD must each state a requirement for: initial entry (`from:
None`, `init_child.rs:502` and `:671`, never a self-entry), cross-phase arrival,
advance-loop self-entry, directed self-entry, and any rewind — with `--full` and
`koto status` as the two class-independent overrides; overrides, batch spawning,
retry-on-parent and every non-advancing stop append no entry event and are not
arrivals at all. `Transitioned.from` is `None` only at the two initial-entry
sites; every genuine phase-to-phase move sets it, and `condition_type` is `"auto"`
at init so `from` is the only initial-entry marker. The two shapes most likely to
break a naively-phrased rule are `P → Q → P` inside one tick (starts and ends in
`P`, but is a real cross-phase arrival that must deliver) and `Rewound { from: P,
to: P }` (reachable whenever the last two entry events name the same phase, and
must deliver); cycle detection bounds advance-loop self-entries to at most one per
tick, always as the tick's first hop, because `visited` is empty on entry and
every state is inserted the moment it is entered.
