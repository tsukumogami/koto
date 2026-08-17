# Lead: Does a self-transition leave any distinguishable trace in the event log other than the transition event itself, and could the way `koto rewind` appends events (koto#199) cause the delivery predicate to regress once self-entry stops opening an occupancy?

Read against branch `docs/self-loop-suppresses-details` (worktree of `tsukumogami/koto`), based on merged `main` b7b0799 ("feat(engine): deliver phase instructions on a rule an agent can rely on (#197)").

## Findings

### 1. Complete session event schema

The enum is `EventPayload` in `src/engine/types.rs:454-832`. It is `#[serde(untagged)]`; the discriminant lives on the outer `Event.event_type` string, and the outer `Event`'s hand-written `Deserialize` impl (`src/engine/types.rs:1114-1189`) dispatches on the `type` string, falling through to `Unknown` for anything it does not recognize. The wire-name table is `EventPayload::type_name()` at `src/engine/types.rs:1014-1050`.

The envelope is `Event` (`src/engine/types.rs:1068-1093`): `seq: u64`, `timestamp: String`, `event_type: String`, `payload: EventPayload`, `idempotency_hash: Option<String>`.

| Variant | wire `type` | Fields | Carries a SOURCE phase? |
|---|---|---|---|
| `WorkflowInitialized` | `workflow_initialized` | `template_path: String`, `variables: HashMap<String,String>`, `spawn_entry: Option<SpawnEntrySnapshot>` | no phase at all |
| `Transitioned` | `transitioned` | **`from: Option<String>`**, **`to: String`**, `condition_type: String`, `skip_if_matched: Option<BTreeMap<..>>` | **YES — optional** |
| `EvidenceSubmitted` | `evidence_submitted` | `state: String`, `fields: HashMap<String,Value>`, `submitter_cwd: Option<PathBuf>` | target only (`state`) |
| `IntegrationInvoked` | `integration_invoked` | `state`, `integration`, `output` | target only |
| `DirectedTransition` | `directed_transition` | **`from: String`**, **`to: String`**, `rationale: Option<String>` | **YES — required** |
| `Rewound` | `rewound` | **`from: String`**, **`to: String`**, `rationale: Option<String>` | **YES — required** |
| `ContextAdded` | `context_added` | `key`, `hash`, `size` | no phase |
| `ContextRemoved` | `context_removed` | `key` | no phase |
| `WorkflowCancelled` | `workflow_cancelled` | `state`, `reason` | target only |
| `DefaultActionExecuted` | `default_action_executed` | `state`, `command`, `exit_code`, `stdout`, `stderr` | target only |
| `DecisionRecorded` | `decision_recorded` | `state`, `decision` | target only |
| `GateEvaluated` | `gate_evaluated` | `state`, `gate`, `output`, `outcome`, `timestamp` | target only |
| `GateOverrideRecorded` | `gate_override_recorded` | `state`, `gate`, `rationale`, `override_applied`, `actual_output`, `timestamp` | target only |
| `SchedulerRan` | `scheduler_ran` | `state`, `tick_summary`, `timestamp` | target only |
| `BatchFinalized` | `batch_finalized` | `state`, `view`, `timestamp`, `superseded_by: Option<SupersededByRef>` | target only |
| `ChildCompleted` | `child_completed` | `child_name`, `task_name`, `outcome: TerminalOutcome`, `final_state`, `result: Option<WorkflowResult>` | no phase (child's, not parent's) |
| `IntentUpdated` | `intent_updated` | `intent` | no phase |
| `RequestStoreResult` | `request_store.result` | `result: WorkflowResult` | no phase |
| `RequestCreated` | `request.created` | `request_id`, `requested_by`, `coordinator_of_record`, `legs: BTreeMap<..>`, `inputs: Option<Value>` | no phase |
| `RequestLegBound` | `request.leg_bound` | `request_id`, `leg_name`, `child_session_id`, `dispatch_epoch: Option<u32>`, `issued_by: Option<String>` | no phase |
| `RequestLegProgress` | `request.leg_progress` | `request_id`, `leg_name`, `content: BTreeMap<..>`, `issued_by` | no phase |
| `RequestLegResult` | `request.leg_result` | `request_id`, `leg_name`, `result`, `source: LegResultSource`, `issued_by` | no phase |
| `RequestLegAbandoned` | `request.leg_abandoned` | `request_id`, `leg_name`, `rationale`, `issued_by` | no phase |
| `RequestClosed` | `request.closed` | `request_id`, `disposition: CloseDisposition`, `issued_by` | no phase |
| `InstructionsDelivered` | `instructions_delivered` | `state: String` | target only |
| `Unknown` | (original string preserved) | `type_name`, `raw_payload` | n/a |

**The three phase-source-carrying variants are exactly the three state-entry variants** (`Transitioned`, `DirectedTransition`, `Rewound`) — the same three `occupancy_slice` matches on. Every other phase-bearing variant carries only a single `state` (the phase it happened in).

Two things matter for the predicate design:

- `Transitioned.from` is `Option<String>`. It is `None` for the very first transition into the initial state, written at `koto init` time (`src/cli/init_child.rs:502-507` and `src/cli/init_child.rs:671-676`, both `from: None, to: initial_state, condition_type: "auto"`). Every `from: None` in production is therefore an initial-entry event, never a self-transition. A `None` source must read as "a different phase" (deliver).
- `DirectedTransition.from` and `Rewound.from` are non-optional `String`, so `from == to` is directly and cheaply testable on both.

### 2. What each operation appends, in order

The append sites relevant here are the closure passed into `advance_until_stop` (`src/cli/mod.rs:4049-4059`) and the direct `backend.append_event` calls in `handle_next` / `handle_rewind`.

**(a) `koto next` producing a normal transition A -> B**

1. `EvidenceSubmitted { state: A, ... }` — only if the caller passed `--with-data` / evidence. `src/cli/mod.rs:3816-3820`, appended *before* the advance loop.
2. `DefaultActionExecuted { state: A, ... }` — if A declares a `default_action` (`src/cli/mod.rs:4025`).
3. `GateEvaluated { state: A, gate, outcome, ... }` — one per non-overridden gate on A. `src/engine/advance.rs:382-390`.
4. `Transitioned { from: Some(A), to: B, condition_type: "auto" | "skip_if", skip_if_matched }` — `src/engine/advance.rs:545-551` (normal resolution) or `src/engine/advance.rs:503-509` (skip_if).
5. Loop re-enters at B: gates of B evaluated -> more `GateEvaluated { state: B, .. }`, action of B -> `DefaultActionExecuted { state: B, .. }`, and so on until a stop reason.
6. Optional `SchedulerRan` / `BatchFinalized` if the final state has a `materialize_children` hook (`src/cli/mod.rs:4392`, `4449`).
7. `InstructionsDelivered { state: B }` — appended **after** the response has been printed, only when `resp.carries_details()` (`src/cli/mod.rs:4592-4613`).

**(b) `koto next` producing a self-transition P -> P**

Identical shape, with one thing worth stating precisely. `advance_until_stop` deliberately does **not** seed `visited` with the starting state (`src/engine/advance.rs:210-213`):

> "The starting state is NOT added to visited. The visited set tracks states we've auto-advanced THROUGH during this invocation. The starting state was already arrived at before this invocation, so re-visiting it (e.g., in a review -> implement -> review loop) is legitimate."

So the first iteration resolves P -> P freely and appends `Transitioned { from: Some(P), to: P, condition_type: "auto" }`. Then `visited.insert(P)`, `state = P`, `advanced = true`, `current_evidence` cleared, `fresh_evidence = false`, and the loop re-enters at P. On that second pass **the gates of P are evaluated a second time and a second set of `GateEvaluated { state: P, .. }` events is appended**, and a `default_action` on P **runs a second time** with a second `DefaultActionExecuted`. The second pass then either stops with `NeedsEvidence` (`src/engine/advance.rs:561`) or, if the conditional self-transition still resolves, hits the cycle check (`src/engine/advance.rs:536-542`) and stops with `CycleDetected` — either way only ONE `Transitioned` event per tick.

Full order for a self-loop tick with evidence and a gate on P:

```
evidence_submitted   { state: P }
gate_evaluated       { state: P }            <- first pass
transitioned         { from: P, to: P }
gate_evaluated       { state: P }            <- second pass, same tick
[ instructions_delivered { state: P } ]      <- today: yes, because occupancy reset
```

**Answer to the lead's first half:** the only trace uniquely identifying a self-transition is the `Transitioned` event's own `from == to`. The duplicated `GateEvaluated` / `DefaultActionExecuted` pair is a *side effect* of the loop re-entering, not a marker: a phase with no gates and no default action produces exactly one event for the whole self-loop tick (the `Transitioned`), and a normal A->B advance also produces two `GateEvaluated` runs (one for A, one for B) — they just name different phases. So `from == to` on the entry event is the only reliable signal, and it is available directly on all three entry variants.

**(c) `koto next` blocked by a gate (no transition)**

`GateEvaluated { state: P, .. }` per gate, then either an immediate `StopReason::GateBlocked` (`src/engine/advance.rs:430-436`, when P has no `accepts` and no `gates.*` routing) or a fall-through to `NeedsEvidence`. **No entry event is appended.** Nothing else. Then `InstructionsDelivered { state: P }` only if the response carried details — which today it does not, because the occupancy has not moved and a delivery record already sits in it. This is the "unchanged, suppress" case.

**(d) `koto next --to X`**

1. `DirectedTransition { from: current, to: X, rationale }` — `src/cli/mod.rs:3336-3345`, appended *before* dispatch and with no gate evaluation (`dispatch_next(target, .., true, &empty_gate_results)`, `src/cli/mod.rs:3359`).
2. Response printed.
3. `InstructionsDelivered { state: X }` if `resp.carries_details()` (`src/cli/mod.rs:3457-3468`).

No `GateEvaluated`, no `EvidenceSubmitted`, no advance loop. The directed path is single-shot.

**(e) `koto next --to P` when already at P**

Identical to (d) with `from == to == P`. It is only legal when P declares a transition targeting itself — the validation at `src/cli/mod.rs:3305-3322` checks `target` against `current_template_state.transitions`. The integration test `directed_transition_into_the_same_phase_carries_details_again` (`tests/instructions_delivery_test.rs:490-514`) exercises exactly this and today asserts the details ARE re-delivered.

Note the elaborate comment at `src/cli/mod.rs:3382-3402` arguing that `already_delivered` "provably evaluates to `false` on every call" on this path, because the synthetic entry event is always the newest element so the occupancy slice is always empty. **That proof stops holding the moment the predicate stops treating a same-phase entry as an occupancy boundary.** The comment (and probably the call site) must be revisited as part of this change; the code will still be correct, but the stated reasoning will be false.

**(f) `koto next --full`**

`--full` changes nothing about which events are appended by the advance loop. It only flips the last argument of `with_details_suppressed_unless_full` (`src/cli/next_types.rs:392-394`: `let suppress = already_delivered && !full;`), so `details` survives, so `carries_details()` is true, so an `InstructionsDelivered` record IS appended. That means `--full` on an already-delivered occupancy appends a **second** (or third) `InstructionsDelivered { state: P }` inside the same occupancy. The predicate is `.any(..)`, so duplicates are harmless — but any future code that counts them, or that asserts one-per-occupancy, would be wrong. The test `override_call_records_a_delivery_so_the_next_plain_call_omits_instructions` (`tests/instructions_delivery_test.rs:554-600`) locks in that `--full` records.

**(g) `koto rewind`**

`handle_rewind` (`src/cli/mod.rs:1985-2089`) appends exactly one event: `Rewound { from, to, rationale }` (`src/cli/mod.rs:2044-2055`). It then re-reads the log and may perform child relocation (which writes to child sessions, not the parent log). **It does not append any `InstructionsDelivered`, and it does not print a directive or details at all** — the response is `{name, state, children, superseded_branch, children_relocated}`. So the details reach the agent on the *next* `koto next`, which sees a fresh occupancy opened by the `Rewound` event.

A second producer of `Rewound` exists: the batch retry path, `write_rewound_event` in `src/cli/retry.rs:533-558`, which appends `Rewound { from: child_current_state, to: child_template_initial_state }` onto a failed child's log.

**(h) `koto status`**

Appends nothing. `handle_status` (`src/cli/mod.rs:4977-...`) reads the log, derives the machine state, and returns `directive`/`details`/`expects` in full, deliberately bypassing the delivery rule. The comment at `src/cli/mod.rs:5069-5076` states it: *"This retrieval always returns the full instructions regardless of the delivery rule `koto next` applies (PRD R10), and it appends nothing: no delivery record, no other event, and no lock is taken anywhere in this function."* This is the escape hatch for any case the new predicate suppresses.

### 3. What `Rewound` records, and whether a self-rewind is reachable

There is no `src/engine/rewind.rs`. Rewind lives in the CLI: `handle_rewind` at `src/cli/mod.rs:1985`.

`Rewound` records **both** endpoints (`src/cli/mod.rs:2044-2048`):

```rust
let rewind_payload = EventPayload::Rewound {
    from: from_state.clone(),   // the phase being LEFT
    to: prev_state.clone(),     // the phase being ENTERED
    rationale,
};
```

`from_state` is `derive_state_from_log(&events)` — the phase the session currently occupies (`src/cli/mod.rs:2031`, `2042`). `to` is computed as:

```rust
let prev_event = state_changing[state_changing.len() - 2];
let prev_state = match &prev_event.payload {
    EventPayload::Transitioned { to, .. } => to.clone(),
    EventPayload::DirectedTransition { to, .. } => to.clone(),
    EventPayload::Rewound { to, .. } => to.clone(),
    _ => unreachable!(),
};
```

That is: **the `to` of the second-most-recent state-entry event**, not the `from` of the most recent one. This is both the cause of #199 and the reason self-rewinds exist.

**A rewind that lands on the phase the session is already in is reachable, by three routes.**

*Route 1 — rewind immediately after a self-transition.* The `to` of the second-most-recent entry event equals the current state exactly when the last two entry events name the same target, which is precisely what a self-transition creates. Concretely:

```
seq 2: transitioned        { from: null, to: "a" }      <- koto init
seq 5: transitioned        { from: "a",  to: "a" }      <- self-loop
       state_changing.len() == 2, so rewind is allowed (guard is <= 1, src/cli/mod.rs:2024)
       current_state = "a";  prev_event = seq 2;  prev_state = "a"
seq 6: rewound             { from: "a",  to: "a" }      <- SELF-REWIND
```

Same for `koto next --to P` while at P, followed by `koto rewind`.

*Route 2 — the batch retry path.* `write_rewound_event` (`src/cli/retry.rs:533-558`) targets the child's `initial_state`. A child that fails while still in its initial phase yields `from == to`. The code even names the degenerate case in its fallback comment: *"Fallback: use current_state as target (vacuous rewind) rather than crashing"* (`src/cli/retry.rs:539-540`).

*Route 3 — #199's oscillation combined with a self-loop.* See sub-question 4.

**So: if the predicate becomes "an occupancy opens only on entry from a DIFFERENT phase", a self-rewind stops opening an occupancy and the following `koto next` SUPPRESSES the instructions.** That directly contradicts the stated intent "Rewind into P from elsewhere: DELIVER (a rewind is a 'redo this' signal)" — a self-rewind is a redo signal too, arguably the strongest one, since the operator typed `koto rewind` explicitly. Deciding this is the single most important open question below.

### 4. koto#199 — the rewind oscillation, and its interaction with the new predicate

The issue (`tsukumogami/koto#199`, open, filed by dangazineu, no comments) reports that successive rewinds bounce between two states instead of walking back: `c -> b -> c -> b`. It ends with a note aimed squarely at this work:

> "Related: the delivery rule shipped in #197 treats a `Rewound` event as beginning a new occupancy, so a rewind arrival correctly re-delivers a phase's instructions. That behaviour is orthogonal to this bug and is unaffected by whichever fix lands here — but a fix that changes which events rewind appends should re-check `instructions_delivered_this_occupancy`."

The issue's guessed cause is correct and I confirmed it from the code. `state_changing[len-2].to` is the target. After the first rewind the log is `[.., T(b->c), Rewound(c->b)]`, so `len-2` is `T(b->c)` whose `to` is `c` — the second rewind goes forward to `c`. And so on.

**Does the oscillation change which events get appended in a way that interacts with the new predicate?**

Not in itself. Every event in an oscillation is a `Rewound` with `from != to` (`c->b`, `b->c`, `c->b`, ...), so under the new predicate every one still opens an occupancy and every subsequent `koto next` still delivers. The oscillation is a wrong-destination bug, not a wrong-event bug. On this narrow reading the issue's own assessment holds: orthogonal.

**But there are two real interaction risks a fix for #199 would create, and one that exists already:**

1. **A #199 fix that emits a no-op rewind becomes an instruction blackout.** The natural fix — track a walk-back cursor, and when the cursor has nowhere further to go, stay put — has an obvious implementation that appends `Rewound { from: X, to: X }` and reports "already at the initial state". Under the current predicate that is harmless (fresh occupancy, re-deliver). Under the new predicate it silently suppresses. Whoever fixes #199 must be told to **refuse (error) rather than append a vacuous `Rewound`**, or the two changes compose into a bug neither introduced alone. This is a coupling worth writing into #199 as a comment.

2. **Self-loop plus rewind already produces a self-rewind today** (route 1 above), independent of any #199 fix, and #199's root cause is what produces it. If #199 is fixed by making rewind walk back through the `from` chain rather than the `to` chain, route 1 disappears for free: `Rewound { from: a, to: <the from of the last entry event> }` — and the `from` of a self-transition is still `a`, so actually it does NOT disappear. A correct walk-back has to skip over self-transitions explicitly to avoid a vacuous hop. Flag this to whoever takes #199.

3. **The retry path (route 2) is unaffected by any #199 fix** — it constructs its `Rewound` directly and will keep producing `from == to` for a child that failed in its initial phase.

### 5. Schema version, header, and backward compatibility

Yes, the session log has a header line. `StateFileHeader` (`src/engine/types.rs:218-...`) is the first line of the state file and carries `schema_version: u32` with `#[serde(default = "default_schema_version")]`, where `default_schema_version()` returns a **fixed literal 1**, deliberately not `CURRENT_SCHEMA_VERSION` (`src/engine/types.rs:201-215`).

`CURRENT_SCHEMA_VERSION` is **1** (`src/engine/types.rs:199`), asserted by a test at `src/engine/types.rs:3423`. The gate is enforced generically through the `LogHeader` trait (`src/engine/persistence.rs:42-57`): readers reject a log whose `schema_version` exceeds `max_supported_schema_version()` with `EngineError::IncompatibleSchemaVersion`, and the check runs before any event is parsed. Request logs version independently via `request_store::REQUEST_SCHEMA_VERSION`.

There is no migration code, because there has never been a migration. The compatibility mechanism is forward-tolerance, documented in `docs/STABILITY.md:190-238`:

- **Adding an event variant is additive and does NOT bump the version.** An older reader lands it in `EventPayload::Unknown { type_name, raw_payload }` (the fall-through arm at `src/engine/types.rs:1367-1370`) and round-trips it byte-identically on write, because `Event::Serialize` writes back `self.event_type` rather than `payload.type_name()` (`src/engine/types.rs:1102-1105`). The `Unknown` arm has existed since v0.9.0, which is the back-compat floor.
- **Adding an optional field to an existing variant is additive** provided it is `#[serde(default, skip_serializing_if = "Option::is_none")]` — that is the house idiom, used on `spawn_entry`, `skip_if_matched`, `submitter_cwd`, `rationale`, `superseded_by`, `result`, `dispatch_epoch`, `issued_by`, `idempotency_hash`. `docs/STABILITY.md:66-80`: minor and patch releases raise the constant by 0.
- Bumping is required only for a new *required* event type, removal of a required field, or an envelope-key change (`src/engine/types.rs:192-198`).

**Practical consequence for this change:** if the predicate can be expressed purely as a read over `from`/`to` on the existing three entry variants — which sub-question 1 shows it can — **no schema change of any kind is needed**. If a new variant or field is judged necessary, it is still additive and `CURRENT_SCHEMA_VERSION` stays at 1; the existing `EventPayload::InstructionsDelivered` addition in #197 is the precedent (`src/engine/types.rs:797`: *"Additive per `docs/STABILITY.md`, so it does not move `CURRENT_SCHEMA_VERSION`"*).

The one asymmetry: a session written by the **new** build and read by the **current release** binary will be interpreted under the old predicate. Since the new predicate only *narrows* delivery, an old binary reading a new log simply delivers more often — a benign direction.

### 6. "What phase was the session in before the current one" — existing helper?

**There is no such helper.** The closest things are:

- `derive_state_from_log(events)` (`src/engine/persistence.rs:708-715`) — the CURRENT phase, the `to` of the last entry event. No "previous" counterpart exists.
- `handle_rewind`'s inline `state_changing[len - 2]` computation (`src/cli/mod.rs:2012-2038`) — the only code that reaches for a prior phase, and it is (a) inline, (b) not a function, and (c) the source of #199. Do not reuse it.
- `build_event_summary` (`src/cli/dashboard_data.rs:830-847`) destructures `from` on all three entry variants purely for display.

**The cheapest correct way to compute it is not to compute it at all.** The `from` field is already on the entry event. The predicate does not need "the previous phase" as a derived quantity — it needs "did the entry event that opened this occupancy come from a different phase", which is a single field comparison on one event the scan already visits:

```rust
// helper on the payload, next to type_name()
fn entry_source_and_target(p: &EventPayload) -> Option<(Option<&str>, &str)> {
    match p {
        EventPayload::Transitioned { from, to, .. } => Some((from.as_deref(), to.as_str())),
        EventPayload::DirectedTransition { from, to, .. } => Some((Some(from.as_str()), to.as_str())),
        EventPayload::Rewound { from, to, .. } => Some((Some(from.as_str()), to.as_str())),
        _ => None,
    }
}
```

An entry event "opens a delivery occupancy for P" iff `target == P && source != Some(P)` — with `source == None` (initial entry) counting as different, which is what you want.

**Critical implementation constraint: do NOT change `occupancy_slice` itself.** `occupancy_slice` (`src/engine/persistence.rs:1028-1046`) is deliberately shared between `instructions_delivered_this_occupancy` and `latest_epoch_gate_failed` — its doc comment says so explicitly (`src/engine/persistence.rs:1017-1019`): *"Shared rather than copied so the predicates built on it -- the epoch-scoped gate classification and the delivery check -- cannot come to disagree about where an occupancy starts."* Changing it would silently move the dashboard's and the `/workflows` projection's "blocked" classification: a gate that failed before a self-loop would start counting as current-epoch.

Worse, the same "last entry event naming the current state" scan is **open-coded five more times** with the old semantics and would not follow:

- `derive_evidence` — `src/engine/persistence.rs:729-740`
- `derive_decisions` — `src/engine/persistence.rs:765-778`
- `derive_overrides` — `src/engine/persistence.rs:803-816`
- `derive_last_gate_evaluated` — `src/engine/persistence.rs:848-861`
- `dashboard_data::read_detail` — `src/cli/dashboard_data.rs:655-671`

A self-transition must keep clearing evidence, decisions, and gate overrides (that is what makes the second loop iteration re-evaluate from scratch). So the delivery rule needs its **own** slice function — call it `delivery_occupancy_slice` — sitting beside `occupancy_slice`, with a doc comment saying exactly why the two differ. The shared-definition argument in the existing comment then needs amending rather than deleting: gate-epoch and delivery-occupancy are now genuinely different questions.

## Implications

1. **The change is a pure read-side predicate change. No new event, no new field, no schema bump.** Everything needed is already on the wire: `from` is present and non-optional on `DirectedTransition` and `Rewound`, and `Option<String>` on `Transitioned` where `None` unambiguously means initial entry.

2. **Fork the slice, do not edit it.** `occupancy_slice` must keep its current semantics for `latest_epoch_gate_failed`; a new `delivery_occupancy_slice` (same scan, skipping entry events whose `from == to`) backs `instructions_delivered_this_occupancy`. Anything else silently changes gate-blocked classification in the dashboard and the `/workflows` projection.

3. **The directed-path call site's correctness proof breaks.** `src/cli/mod.rs:3382-3402` argues at length that `already_delivered` is provably `false` there because the just-appended entry event is always the newest element. Under the new predicate a `--to P` while at P no longer opens an occupancy, so the slice reaches back past it and the check can return `true` — which is the desired new behaviour, but the comment becomes actively misleading and must be rewritten. The path already builds its post-append event list in memory (no extra file read), so no I/O changes.

4. **Three tests encode the old behaviour and must flip:** `self_transition_arrival_carries_details_again` (`tests/instructions_delivery_test.rs:358-380`), `directed_transition_into_the_same_phase_carries_details_again` (`tests/instructions_delivery_test.rs:487-514`), and the unit test `instructions_delivered_resets_on_a_self_transition` (`src/engine/persistence.rs:2595-2609`). A fourth, `tests/next_response_baseline.rs:362`, has a case described as *"`implement` transitions to itself, ending one occupancy and beginning another"* — its expected output changes too. `loop_back_arrival_at_previously_occupied_phase_carries_details_again` (`tests/instructions_delivery_test.rs:382-406`) and `rewind_arrival_carries_details` must NOT change; they are the guard rails proving the change did not over-reach.

5. **A self-rewind must be decided explicitly, and it is reachable today.** `koto rewind` right after a self-loop produces `Rewound { from: P, to: P }` with no #199 fix required. Under a naive `from != to` rule the operator's explicit redo request produces a response with no instructions. My recommendation: make the rule **`from != to` for `Transitioned` and `DirectedTransition`, but treat every `Rewound` as opening an occupancy regardless of `from`/`to`.** It matches the stated semantics ("a rewind is a 'redo this' signal"), it is one line, it removes the entire #199 coupling, and it keeps the retry path's vacuous rewind (`src/cli/retry.rs:539`) delivering. The cost is that a `Rewound { P -> P }` is not literally "entry from a different phase" — but it is unambiguously an operator-initiated redo, which is the thing AC 3 is not trying to suppress.

6. **File a note on koto#199.** Whoever fixes it must not implement "already at the initial state" as an appended vacuous `Rewound` — under the new rule that becomes an instruction blackout. If recommendation 5 is adopted this risk evaporates, which is a further argument for it.

7. **`koto status` is the safety valve.** It appends nothing, takes no lock, and returns full `details` regardless of the delivery rule (`src/cli/mod.rs:5069-5076`). Every case the new predicate suppresses stays recoverable, and the recovery pointer spliced into the directive on every instruction-carrying phase (`src/cli/mod.rs:4310-4314`, `3426-3430`) already tells the agent about it. That is what makes narrowing delivery safe.

## Surprises

- **`docs/reference/session-feed.md` is stale in the same commit that made it wrong.** PR #197 added the `instructions_delivered` section, including *"**Not emitted yet.** The event type is reserved and its shape is fixed, but no koto build appends one: instruction suppression is still keyed on visit count. A reader will not find this event in any session log written today"* (`docs/reference/session-feed.md:687-691`) — while the same commit shipped the two append sites at `src/cli/mod.rs:3459` and `src/cli/mod.rs:4604`. The doc section was clearly written for an earlier issue in the plan and never updated when the wiring landed. The identical claim sits in the Rust doc comment at `src/engine/types.rs:770-772` (*"**Nothing appends this yet.**"*). Both are wrong today and should be corrected as part of this change — they are the first thing a reader of the feed contract will consult.

- **A self-loop tick runs the phase's gates twice and its `default_action` twice.** Because `advance_until_stop` re-enters the loop at the new state and the new state is the same state, a self-transitioning phase with a gate appends two `GateEvaluated { state: P }` events per tick, and a phase with a `default_action` executes the shell command twice per tick. This is pre-existing and out of scope, but it means "count of `GateEvaluated` in the epoch" is not a stable quantity across self-loops, and it is worth knowing before anyone reaches for gate events as a proxy signal.

- **`--full` appends a duplicate `InstructionsDelivered` into an occupancy that already has one.** `carries_details()` is evaluated after suppression is overridden, so the record is written unconditionally. Harmless under `.any(..)`, and deliberately locked in by a test, but it means the record is not one-per-occupancy and no future code should assume it is.

- **`handle_rewind` computes its destination from the second-most-recent entry event's `to`, not from the most recent event's `from`** — even though `from` is right there on the payload. Using `from` would fix #199 in about three lines. I did not investigate further per the constraint, but the fix looks much smaller than the issue text implies.

- **Five open-coded copies of the epoch-boundary scan** exist alongside the one shared `occupancy_slice`, all with identical logic and near-identical comments. Any predicate change made in the wrong place will diverge from four of them silently.

## Open Questions

1. **Should a `Rewound { from: P, to: P }` open a delivery occupancy?** This needs a human ruling. It is reachable today via self-loop-then-rewind and via the batch retry path, and the two answers give materially different behaviour for an operator who explicitly asked to redo a phase. My recommendation is yes (treat all `Rewound` as opening), but the parent should decide.
2. **What should `koto next --to P` while at P mean, exactly?** The exploration context says SUPPRESS, and the mechanism is clear. But it is an explicit operator directive, the same class of signal as a rewind. Confirm the asymmetry with rewind is intended rather than accidental.
3. **Does the `advanced: true` flag on a self-loop response need to change?** Out of scope for this lead, but a response that says `advanced: true` and carries no `details` is a shape no agent has seen before. Worth a look from whoever owns the response contract.
4. Should the correction to `docs/reference/session-feed.md` and `src/engine/types.rs`'s "not emitted yet" claims ride this PR or go separately? They are wrong on `main` right now.

## Summary

The only reliable log trace of a self-transition is the `from == to` on its own entry event, and that field is already present on all three state-entry variants (`Transitioned.from` is `Option<String>`, `None` only for initial entry; `DirectedTransition.from` and `Rewound.from` are required), so the whole change is a read-side predicate with no schema bump — but it must go in a NEW `delivery_occupancy_slice` rather than in the shared `occupancy_slice`, which also backs gate-epoch classification and has five open-coded twins that would not follow. The predicate does regress on one reachable case: `koto rewind` immediately after a self-loop appends `Rewound { from: P, to: P }` (because rewind targets `state_changing[len-2].to`, the same root cause as #199), so a naive `from != to` rule would give an operator's explicit redo request a response with no instructions — I recommend exempting `Rewound` from the sameness test entirely, which also decouples this work from whatever fix #199 gets. The biggest open question is exactly that: whether a self-rewind should re-deliver, which needs a human ruling before the predicate is written.
