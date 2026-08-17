# Lead: How does the phase-details delivery predicate actually decide today, and where in that evaluation is the *previous* phase knowable?

Everything below was read from the worktree at commit `b7b0799` ("feat(engine):
deliver phase instructions on a rule an agent can rely on (#197)").

## Findings

### 1. The predicate and the shared slice helper

Two functions in `src/engine/persistence.rs` do the whole job.

`occupancy_slice` (`src/engine/persistence.rs:1028-1045`) is private and shared:

```rust
fn occupancy_slice<'a>(events: &'a [Event], current_state: &str) -> &'a [Event] {
    let start = events.iter().enumerate().rev().find_map(|(idx, e)| {
        let to = match &e.payload {
            EventPayload::Transitioned { to, .. } => Some(to.as_str()),
            EventPayload::DirectedTransition { to, .. } => Some(to.as_str()),
            EventPayload::Rewound { to, .. } => Some(to.as_str()),
            _ => None,
        };
        if to == Some(current_state) {
            Some(idx)
        } else {
            None
        }
    });
    match start {
        Some(idx) => &events[idx + 1..],
        None => events,
    }
}
```

Note precisely what it matches on: **only `to`**. The `from` field is destructured
away with `..` in all three arms. The search runs backwards (`.rev()`), takes the
*last* entry event naming `current_state`, and returns everything strictly after
it. With no such event it returns the whole log (the initial-state fallback).

Its doc comment (`src/engine/persistence.rs:997-1027`) states the current
semantics outright:

> An occupancy begins when a state-entry event names the phase as its target
> and ends when the next state-entry event names any phase, including the same
> one. So a self-transition ends one occupancy and begins another, and so does
> a rewind into the phase: both append an entry event naming it. State-entry
> events are `Transitioned`, `DirectedTransition`, and `Rewound`.

and, importantly for the change:

> Shared rather than copied so the predicates built on it -- the epoch-scoped
> gate classification and the delivery check -- cannot come to disagree about
> where an occupancy starts.

The delivery predicate (`src/engine/persistence.rs:1099-1106`):

```rust
pub fn instructions_delivered_this_occupancy(events: &[Event], current_state: &str) -> bool {
    occupancy_slice(events, current_state).iter().any(|e| {
        matches!(
            &e.payload,
            EventPayload::InstructionsDelivered { state } if state == current_state
        )
    })
}
```

So the conclusion is: "was there an `InstructionsDelivered` naming this exact
phase, positioned after the most recent entry event naming this phase?" The name
check is belt-and-braces for the whole-log fallback and for a stray record from
another phase landing inside the window (documented at
`src/engine/persistence.rs:1090-1096`); it errs toward re-delivering.

The **other** consumer of the same slice is `latest_epoch_gate_failed`
(`src/engine/persistence.rs:1057-1068`), which decides "is the dashboard's
blocked classification true for this phase" from the latest `GateEvaluated` in
the slice. Its callers are `src/cli/dashboard_data.rs:458` and
`src/workflows_surface/project.rs:183`.

**Events in the JSONL log.** `EventPayload` is an untagged enum in
`src/engine/types.rs:454-831`. The variants are: `WorkflowInitialized`,
`Transitioned`, `EvidenceSubmitted`, `IntegrationInvoked`, `DirectedTransition`,
`Rewound`, `ContextAdded`, `ContextRemoved`, `WorkflowCancelled`,
`DefaultActionExecuted`, `DecisionRecorded`, `GateEvaluated`,
`GateOverrideRecorded`, `SchedulerRan`, `BatchFinalized`, a child-terminal
notification variant, `InstructionsDelivered`, and `Unknown`. The envelope is
`Event { seq, timestamp, event_type, payload, idempotency_hash }`
(`src/engine/types.rs:1068-1093`); dispatch on read goes through the `type` string,
not serde's untagged matcher.

### 2. Every state-entry variant, verbatim, with its `from`/`to` fields

All three are struct variants of `EventPayload` in `src/engine/types.rs`.

`Transitioned` (`src/engine/types.rs:466-480`):

```rust
    Transitioned {
        from: Option<String>,
        to: String,
        condition_type: String,
        /// When the transition was triggered by a `skip_if` condition,
        /// this field records the matched key-value pairs from the
        /// state's `skip_if` map. `None` for ordinary evidence-driven
        /// or gate-driven transitions.
        ///
        /// Additive field: omitted from serialization when `None` so
        /// pre-feature JSONL files round-trip without modification.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip_if_matched: Option<BTreeMap<String, serde_json::Value>>,
    },
```

`DirectedTransition` (`src/engine/types.rs:501-507`):

```rust
    DirectedTransition {
        from: String,
        to: String,
        /// Optional human-readable reason for this directed transition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
```

`Rewound` (`src/engine/types.rs:508-514`):

```rust
    Rewound {
        from: String,
        to: String,
        /// Optional human-readable reason for this rewind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
```

Source phase = `from` on all three; target phase = `to` on all three. Only
`Transitioned::from` is optional, and it is `None` exactly once per session: the
synthetic initial transition written at init time (`src/cli/init_child.rs:502-506`
uses `from: None, to: initial_state`; the same shape appears in the top-level init
path and in test fixtures at `src/cli/overrides.rs:489`, `src/session/local.rs:1425`,
`src/workflows_surface/materialize.rs:366`).

Production writers that populate `from: Some(...)`: the advance loop's two append
sites, `src/engine/advance.rs:503-508` (skip_if path) and
`src/engine/advance.rs:545-550` (ordinary path), both `from: Some(state.clone())`
where `state` is the phase being left.

`DirectedTransition` is written in exactly one production place,
`src/cli/mod.rs:3336-3340`:

```rust
        let payload = EventPayload::DirectedTransition {
            from: current_state.clone(),
            to: target.clone(),
            rationale: rationale.clone(),
        };
```

`Rewound` is written in two production places: `koto rewind`'s handler
(`src/cli/mod.rs:2044-2048`, `from: from_state.clone(), to: prev_state.clone()`)
and the retry path's child rewind (`src/cli/retry.rs:551-555`).

### 3. Can the code tell "entered from a different phase" from "entered from the same phase"?

**Yes. The `from` field is recorded on every state-entry event, and
`occupancy_slice` simply throws it away.**

- `EventPayload::Transitioned::from: Option<String>` — `Some(source)` for every
  transition the advance loop appends; `None` only for the init event, which by
  construction is not a self-loop.
- `EventPayload::DirectedTransition::from: String` — always the phase the
  `--to` handler was standing on.
- `EventPayload::Rewound::from: String` — always the phase being rewound out of.

So the information needed for the AC-3 semantics is already on disk and already
in the `&[Event]` slice the predicate receives. No schema change, no new event,
no migration. A self-loop `P -> P` is exactly `from == Some(P)/P && to == P`.

Concretely, the change is confined to the `find_map` closure: instead of matching
on `to == Some(current_state)`, it needs to match on `to == current_state && from
!= current_state`. Existing sessions written by `b7b0799` replay correctly under
the new rule because `from` was already being written.

**But the helper is shared.** Changing `occupancy_slice` in place also changes
`latest_epoch_gate_failed`, and therefore the dashboard/`/workflows` blocked
classification: after a self-transition `P -> P`, gate evaluations from *before*
the self-transition would come back into the epoch window. Whether that is
desirable is a separate question from AC 3. The two realistic shapes are (a) add
a `from`-aware parameter or a second slicing function used only by the delivery
predicate, accepting that the "cannot come to disagree" comment at
`src/engine/persistence.rs:1017-1020` no longer holds literally, or (b) change the
shared helper and accept the gate-epoch consequence deliberately. This is the one
genuine design decision the change carries.

Also worth knowing: a natural self-transition really does append an event. The
advance loop's `visited` set deliberately does **not** contain the starting state
(`src/engine/advance.rs:187`, and the comment at `src/engine/advance.rs:210-213`:
"The starting state is NOT added to visited ... re-visiting it (e.g., in a review
-> implement -> review loop) is legitimate"). So `P -> P` passes the cycle check
on the first hop, appends `Transitioned { from: Some(P), to: P }`, inserts `P`
into `visited`, and a second consecutive self-hop in the *same* tick would then
trip `StopReason::CycleDetected`. One tick, one self-entry event.

### 4. The delivery record and `--full`

The event (`src/engine/types.rs:808-811`):

```rust
    InstructionsDelivered {
        /// The phase whose instructions the response carried.
        state: String,
    },
```

`type_name()` maps it to `"instructions_delivered"` (`src/engine/types.rs:1040`);
deserialization goes through `InstructionsDeliveredPayload { state: String }`
(`src/engine/types.rs:1456-1458`). Its doc comment notes it is declared last among
the real variants on purpose, because the enum is `#[serde(untagged)]` and a
`{state}`-only variant would otherwise shadow `WorkflowCancelled` and
`DecisionRecorded`.

A delivery is recorded by both `koto next` paths, **after** the response has been
printed, and only when the printed response actually carried `details`. The gate
is `NextResponse::carries_details()` (`src/cli/next_types.rs:496-505`), which is
evaluated *after* suppression has been applied — so it is `details.is_some()` on
the post-suppression response.

Natural path (`src/cli/mod.rs:4600-4613`):

```rust
            if resp.carries_details() {
                let ts = now_iso8601();
                let payload = EventPayload::InstructionsDelivered {
                    state: final_state.clone(),
                };
                if let Err(e) = backend.append_event(&name, &payload, &ts) {
```

Directed path (`src/cli/mod.rs:4455-4470` region, quoted at `3456-3468`): the same
shape with `state: target.clone()`.

**`koto next --full` does record a delivery.** `--full` is threaded into
`with_details_suppressed_unless_full(already_delivered, full)`
(`src/cli/next_types.rs:392-395`):

```rust
    pub fn with_details_suppressed_unless_full(self, already_delivered: bool, full: bool) -> Self {
        let suppress = already_delivered && !full;
        let strip = |details: Option<String>| if suppress { None } else { details };
```

With `full = true` the details survive, so `carries_details()` is `true`, so the
record is appended. There is an integration test asserting exactly this:
`override_call_records_a_delivery_so_the_next_plain_call_omits_instructions`
(`tests/instructions_delivery_test.rs:556`).

The combinator touches five variants — `EvidenceRequired`, `GateBlocked`,
`Integration`, `IntegrationUnavailable`, `ActionRequiresConfirmation` — and
returns `Terminal` / `Error` unchanged, since neither carries `details`.

### 5. The exact `koto next` code path

Entry point: `handle_next` in `src/cli/mod.rs:2892` (the unix implementation; a
stub at `src/cli/mod.rs:4643` errors out on non-unix). It imports the predicate at
`src/cli/mod.rs:2914`.

There are **two** response-construction sites, and both consult the same predicate
through the same combinator.

**Directed path (`--to`)**, `src/cli/mod.rs:3285` onward:

1. Validate the target is a declared transition of the current phase
   (`src/cli/mod.rs:3303-3323`). Note this is what makes `--to P` while at `P`
   possible at all: the template must declare a self-transition.
2. Append `DirectedTransition { from: current_state, to: target }`
   (`src/cli/mod.rs:3336-3353`), capturing `directed_ts` so the synthetic event
   below carries the persisted timestamp.
3. `dispatch_next(target, target_template_state, true, &gate_results)`
   (`src/cli/mod.rs:3358`) builds the response; `with_substituted_directive`
   applies variable substitution.
4. The predicate, `src/cli/mod.rs:3403-3418`:

```rust
                let already_delivered = if target_template_state.details.is_empty() {
                    false
                } else {
                    let post_events: Vec<Event> = events
                        .iter()
                        .cloned()
                        .chain(std::iter::once(Event {
                            seq: events.len() as u64 + 1,
                            timestamp: directed_ts.clone(),
                            event_type: payload.type_name().to_string(),
                            payload: payload.clone(),
                            idempotency_hash: None,
                        }))
                        .collect();
                    instructions_delivered_this_occupancy(&post_events, target)
                };
                let resp = resp.with_details_suppressed_unless_full(already_delivered, full);
```

   The event list is built in memory rather than re-read, to hold PRD R18 (no
   extra read on this path).
5. Recovery-pointer splice (`src/cli/mod.rs:3424-3428`), abandonment notice, then
   `println!`, then the delivery append.

**Natural-advancement path**, `src/cli/mod.rs:4059` onward:

1. `advance_until_stop` runs; on `Ok`, `final_state` and `final_template_state`
   are resolved (`src/cli/mod.rs:4063-4077`).
2. `details` is built from the template, unconditionally at this stage
   (`src/cli/mod.rs:4091-4095`):

```rust
            let details = if final_template_state.details.is_empty() {
                None
            } else {
                Some(final_template_state.details.clone())
            };
```

3. The response is constructed per `StopReason` (the big `match` ending around
   `src/cli/mod.rs:4586`), each arm passing `details: details.clone()`.
4. `with_substituted_directive`, then the predicate at `src/cli/mod.rs:4291-4300`:

```rust
            let already_delivered = if final_template_state.details.is_empty() {
                false
            } else {
                let post_events = backend
                    .read_events(&name)
                    .map(|(_, evts)| evts)
                    .unwrap_or_default();
                instructions_delivered_this_occupancy(&post_events, final_state)
            };
            let resp = resp.with_details_suppressed_unless_full(already_delivered, full);
```

   Here the log **is** re-read, because the advance loop may have appended
   transitions and gate evaluations since the tick's first read.
5. Recovery pointer (`src/cli/mod.rs:4310-4314`), abandonment notice, batch
   scheduler, envelope assembly, `println!` (`src/cli/mod.rs:4590`), then the
   delivery append (`src/cli/mod.rs:4600`).

So `details` is *attached* at step 2/3 from `final_template_state.details`, and
*omitted* only inside `with_details_suppressed_unless_full`. The single
behavioural lever for this change is what `already_delivered` evaluates to —
which is entirely `occupancy_slice`'s answer.

## Implications

**The change is small and localized.** The `from` phase is already persisted on
all three entry variants, so making self-loops suppress means teaching the slice
search to skip an entry event whose `from` equals the phase being asked about.
Nothing else in the pipeline needs to move: the combinator, the two call sites,
`carries_details()`, the `--full` override and the delivery-record append all keep
working unchanged, and old logs replay correctly.

Walking the six intended cases against a `from`-aware slice:

- `P -> P` natural self-transition: the `Transitioned { from: Some(P), to: P }`
  is skipped, the search continues back to the entry that arrived from elsewhere,
  finds the earlier `InstructionsDelivered { state: P }` inside that window →
  **suppress**. Correct.
- `koto next --to P` while at `P`: the synthetic `DirectedTransition { from: P,
  to: P }` in the in-memory `post_events` is skipped for the same reason →
  **suppress**. Correct. This kills the "provably evaluates to `false` on every
  call" claim in the directed path's long comment
  (`src/cli/mod.rs:3377-3400`) — that comment must be rewritten, because the
  synthetic newest event will no longer always be the occupancy opener.
- Non-advancing re-tick: nothing appended, slice unchanged → **suppress**.
  Unchanged.
- Arrival from a different phase, loop-back from a later phase, rewind from
  elsewhere: `from != P`, entry event matches, slice starts empty → **deliver**.
  Unchanged.
- `--full`: orthogonal, still forces delivery and still records one. Unchanged.

**The shared-helper decision is the real design question.** `occupancy_slice`
currently backs both the delivery predicate and `latest_epoch_gate_failed`, and
its doc comment sells the sharing as a correctness property. Changing it in place
silently redefines the gate epoch across a self-transition
(`src/cli/dashboard_data.rs:458`, `src/workflows_surface/project.rs:183` are the
affected readers). Splitting it means writing down why the two predicates
legitimately disagree.

**Blast radius on tests.** Two unit tests in `src/engine/persistence.rs` invert:
`instructions_delivered_resets_on_a_self_transition`
(`src/engine/persistence.rs:2591-2607`, whose first assertion becomes `true`) and
the second half of
`instructions_delivered_resets_on_arrival_by_directed_transition`
(`src/engine/persistence.rs:2627-2649`). Two integration tests in
`tests/instructions_delivery_test.rs` invert:
`self_transition_arrival_carries_details_again` (line 359, renamed) and
`two_consecutive_directed_transitions_into_same_phase_both_carry` (line 487,
renamed). The `DELIVERY_TEMPLATE` fixture already declares the self-transition
(`implement` with `loop_again: yes`) and the loop-back, so no fixture change is
needed. `loop_back_arrival_at_previously_occupied_phase_carries_details_again`
(line 384) and `rewind_arrival_carries_details` (line 411) must keep passing
untouched — they are the regression guard that the change did not overshoot.

**Doc surface is wide.** Every one of these says a self-transition starts a new
occupancy and must be reworded: `docs/designs/current/DESIGN-inline-phase-details.md`
(lines 30, 220-221, 247-253, 359), `docs/prds/PRD-inline-phase-details.md`
(Definitions at line 140-145, R3 at 166-169, criteria at 261-262 and 273-275),
`docs/guides/cli-usage.md:82` and `:117`, `docs/reference/session-feed.md:683`,
`plugins/koto-skills/skills/koto-user/references/response-shapes.md` (lines 38-45,
107, 168-171, 550), `plugins/koto-skills/skills/koto-user/references/command-reference.md:96`,
`plugins/koto-skills/skills/koto-author/SKILL.md:67`,
`plugins/koto-skills/skills/koto-author/references/template-format.md:118-122`, and
`plugins/koto-skills/.cursor/rules/koto.mdc:171-173`. There is also an evals file,
`plugins/koto-skills/skills/koto-user/evals/evals.json`, that mentions occupancy.

## Surprises

**The PRD explicitly considered this exact conflict and resolved it the other
way.** `docs/designs/current/DESIGN-inline-phase-details.md:245-256` has a section
headed "A contradiction in the PRD was corrected", which says the PRD's
Definitions made a self-transition begin a new occupancy while an acceptance
criterion required a second consecutive directed transition into the same phase
to omit instructions, and concludes: "The Definitions are normative and R3 is
explicit, so the criterion was rewritten to test what it was plainly reaching
for: a directed transition followed by a non-advancing tick." The user's ruling
that AC 3 wins reverses a documented, argued decision — so the design and PRD
need an amendment that says so, not just a wording tweak.

**`docs/reference/session-feed.md` is already stale at `b7b0799`.** Lines 688-689
still say of `instructions_delivered`: "**Not emitted yet.** The event type is
reserved and its shape is fixed, but no koto build appends one: instruction
suppression is still keyed on visit count." That is false as of the merged
commit — `src/cli/mod.rs:4600` and `:3459` both append it, and `derive_visit_counts`
is no longer consulted for this question. Worth fixing in the same change.

**The directed path's comment is unusually load-bearing.** `src/cli/mod.rs:3377-3400`
is a 24-line argument that the predicate "provably evaluates to `false` on every
call" on that path, justified as not-dead-code because it would catch drift if
the append ever stopped being the immediately-preceding event. After the change
that proof is simply wrong, and the call becomes genuinely decision-bearing —
which is arguably a better state for the code, but the comment cannot survive as
written.

**A `Rewound { from: P, to: P }` is reachable.** `koto rewind` picks the
second-to-last state-changing event's `to` as the rewind target
(`src/cli/mod.rs:2031-2037`), so rewinding immediately after a self-transition
targets `P` while standing on `P`. A `from`-aware rule would suppress there. That
is consistent with the ruling ("rewind into `P` from elsewhere: DELIVER" says
nothing about rewinding into `P` from `P`), but it is an unlisted case and should
be decided explicitly rather than fall out.

**`derive_visit_counts` still exists** (`src/engine/persistence.rs:980-995`) and is
still used by `workflows_surface`; it is not part of this predicate any more and
should not be reintroduced into it.

## Open Questions

1. Split `occupancy_slice` or change it in place? Changing it in place moves
   `latest_epoch_gate_failed`'s epoch boundary across self-transitions, affecting
   the dashboard and `/workflows` blocked classification. Needs a deliberate call
   plus a test either way.
2. Should `Rewound { from: P, to: P }` suppress or deliver? A uniform
   `from != to` rule suppresses it; a rule scoped to `Transitioned` /
   `DirectedTransition` only would not.
3. Are the PRD and DESIGN to be amended in place (with a note recording the
   reversal), or superseded? The design's "A contradiction in the PRD was
   corrected" section argues the opposite of the ruling and cannot just be
   deleted quietly.
4. Is `koto status` still the sanctioned recovery route for an agent that wanted
   the re-delivered details on a self-loop? `handle_status` returns
   `directive`/`details`/`expects` unconditionally, and `--full` still forces
   delivery, so the capability is not lost — but the author-facing docs currently
   promise self-loop re-delivery as a template-authoring tool
   (`plugins/koto-skills/skills/koto-author/references/template-format.md:120`),
   and that promise is being withdrawn.

## Summary

The predicate is `instructions_delivered_this_occupancy` at
`src/engine/persistence.rs:1099`, and it decides entirely through the private
`occupancy_slice` helper at `src/engine/persistence.rs:1028`, which matches
state-entry events on `to` alone and discards `from` with `..` — even though
`from` is recorded on all three entry variants (`Transitioned.from:
Option<String>`, `DirectedTransition.from: String`, `Rewound.from: String`) and is
populated by every production writer, so the previous phase is fully knowable at
the exact point the decision is made. Making self-loops suppress therefore needs
no schema change and no migration: it is a `from != current_state` guard in that
one closure, plus updates to two unit tests, two integration tests, the long
"provably false" comment on the directed path at `src/cli/mod.rs:3377-3400`, and
roughly a dozen doc and skill files. The biggest open question is whether to
change the shared `occupancy_slice` in place — which would also move
`latest_epoch_gate_failed`'s gate epoch and the dashboard's blocked
classification — or split the two predicates apart and document why they now
disagree.
