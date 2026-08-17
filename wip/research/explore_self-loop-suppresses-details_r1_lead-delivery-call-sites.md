# Lead: Which call sites decide whether `details` rides along on a response, and what does each one expect?

## Findings

### 1. Every caller of the predicate, and every place `details` is attached or omitted

**The occupancy helper and its two consumers**

`occupancy_slice` is private to `src/engine/persistence.rs`:

- `src/engine/persistence.rs:1028` — `fn occupancy_slice<'a>(events: &'a [Event], current_state: &str) -> &'a [Event]`. Scans backwards for the last event whose payload is `Transitioned`/`DirectedTransition`/`Rewound` with `to == current_state`, returns `&events[idx+1..]`, or the whole log when no such event exists. It **only looks at `to`; `from` is never read**, which is exactly why a self-transition currently resets the occupancy.
- `src/engine/persistence.rs:1059` — `latest_epoch_gate_failed` calls it.
- `src/engine/persistence.rs:1100` — `instructions_delivered_this_occupancy` calls it.

Those are the only two consumers. Nothing else in the tree calls `occupancy_slice`.

**Callers of `latest_epoch_gate_failed`** (the collateral-damage surface):

- `src/cli/dashboard_data.rs:458` — the dashboard read seam's blocked classification.
- `src/workflows_surface/project.rs:183` — the `/workflows` projection writer's blocked classification.

Neither touches `details`. Both would change behaviour if `occupancy_slice` itself were edited.

**Callers of `instructions_delivered_this_occupancy`:**

- `src/cli/mod.rs:3417` — directed-transition path (`koto next --to <state>`).
- `src/cli/mod.rs:4298` — natural-advancement path (plain `koto next`).

Imported at `src/cli/mod.rs:2914`. There are no other callers in `src/` or `tests/`.

**Where `details` is attached to a response**

Two construction regions, both funnelling into one combinator:

- `src/cli/next.rs:50-54` builds `details` for the dispatcher (`dispatch_next`), used by the directed path:
  ```rust
  let details = if template_state.details.is_empty() {
      None
  } else {
      Some(template_state.details.clone())
  };
  ```
  attached at `src/cli/next.rs:64` (`GateBlocked`), `:83` (`IntegrationUnavailable`), `:99` and `:114` (both `EvidenceRequired`).
- `src/cli/mod.rs:4090-4094` builds the same `Option<String>` for the natural path; attached at `src/cli/mod.rs:4132` (`GateBlocked`), `:4153` (`EvidenceRequired`), `:4204` (`ActionRequiresConfirmation`), `:4254` and `:4264` (the `SignalReceived` variants).

**Where `details` is omitted**

One place: `NextResponse::with_details_suppressed_unless_full` at `src/cli/next_types.rs:392`:
```rust
pub fn with_details_suppressed_unless_full(self, already_delivered: bool, full: bool) -> Self {
    let suppress = already_delivered && !full;
    let strip = |details: Option<String>| if suppress { None } else { details };
```
It rewrites the five details-carrying variants (`EvidenceRequired`, `GateBlocked`, `Integration`, `IntegrationUnavailable`, `ActionRequiresConfirmation`) and passes `Terminal`/`Error` through untouched (`:479-480`).

Called from exactly two sites: `src/cli/mod.rs:3419` (directed) and `src/cli/mod.rs:4300` (natural).

Serialization then omits the key entirely rather than emitting `null` — e.g. `src/cli/next_types.rs:516-523`:
```rust
let count = 8 + details.as_ref().map_or(0, |_| 1);
...
if let Some(d) = details {
    map.serialize_entry("details", d)?;
}
```

**Where the delivery record is appended**

- `src/cli/mod.rs:3457-3468` (directed), gated on `resp.carries_details()`, appended *after* `println!`.
- `src/cli/mod.rs:4602-4613` (natural), same gating, same after-print ordering.

`carries_details()` is `src/cli/next_types.rs:492-501` — `details.is_some()` on the five variants, `false` for `Terminal`/`Error`. Because it runs *after* the combinator, `--full` deliveries are recorded and suppressed responses are not.

### 2. How each surface reaches the decision

**`koto next` (natural advancement)** — `src/cli/mod.rs:4291-4300`:
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
Re-reads the log from disk *after* the advance loop, so any `Transitioned`/`GateEvaluated` the loop appended this tick are in scope. `final_state` is `advance_result.final_state`, so the predicate's "must be the currently-occupied phase" precondition holds. The `details.is_empty()` guard keeps instruction-free phases off both the extra read and the extra write (the R6/R18 byte-identity guarantee).

**`koto next --to <state>` (directed)** — `src/cli/mod.rs:3403-3419`. Does *not* re-read; it builds the post-append list in memory:
```rust
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
```
The synthetic event wraps the same `DirectedTransition { from: current_state, to: target }` payload already persisted at `:3336-3345`. Critically for the change: **it carries `from`**, so a self-loop-aware predicate needs no extra plumbing on this path.

The comment at `src/cli/mod.rs:3382-3402` asserts this call "provably evaluates to `false` on every call" because the synthetic entry event is always the newest element. **That proof depends on the current definition of an occupancy boundary and stops holding the moment self-transitions stop being boundaries.** The comment must be rewritten as part of the change; leaving it would actively mislead.

**`koto next --full`** — `full: bool` is a plain clap flag (`src/cli/mod.rs:146-148`, "Always include the details field in the response, regardless of visit count"), threaded into `handle_next` at `:2899` and consumed only inside the combinator. It never touches the predicate: `already_delivered` is still computed, `suppress = already_delivered && !full` just ignores it. Since `carries_details()` runs afterwards, `--full` still appends an `InstructionsDelivered` record. Under the intended change `--full` is unaffected — it forces delivery on a self-loop just as it does on a repeat tick.

**`koto status`** — `handle_status`, `src/cli/mod.rs:4977`. Does not call the predicate at all. See sub-question 5.

**Batch mode (`src/cli/batch.rs`)** — no involvement. The only `details` in the file is a `TemplateState` test fixture at `src/cli/batch.rs:3221`. The scheduler runs at `src/cli/mod.rs:4333-4358`, *after* suppression is applied at `:4300`, and only inserts envelope siblings. Children are separate sessions with separate logs, so a child's first tick hits the natural path against its own log and always delivers. Batch mode needs no insulation.

**Auto-advance loop (`src/engine/advance.rs`)** — never sees `details` (all `details:` hits in that file are `TemplateState` test fixtures) and never appends `InstructionsDelivered`. Its relevance is that it *produces the entry events the predicate reads*:
- `src/engine/advance.rs:534-541` (skip_if path) and `:540-546`/`:...` (`resolve_transition` path) append `EventPayload::Transitioned { from: Some(state), to: target, ... }`.
- Cycle detection (`src/engine/advance.rs:187`, `:210-213`, `:498-504`, `:537-543`) deliberately does *not* seed `visited` with the starting state, so **one** self-transition per tick is legal; a second in the same tick returns `StopReason::CycleDetected`. So `P -> P` in the natural path appends exactly one `Transitioned { from: Some("P"), to: "P" }`, then re-enters the loop at P, evaluates gates again (appending `GateEvaluated`), and stops on `NeedsEvidence`.

Net: today the natural-path self-loop lands on an occupancy slice containing only this tick's `GateEvaluated` events — no `InstructionsDelivered` — so `already_delivered` is `false` and `details` ships again. That is the behaviour AC 3 rules against.

### 3. `koto next --to P` when the session is already at P

**It is only reachable when the template declares a `P -> P` transition.** `src/cli/mod.rs:3304-3322` validates the target against the *current* phase's declared transitions:
```rust
let valid_targets: Vec<&str> = current_template_state
    .transitions
    .iter()
    .map(|t| t.target.as_str())
    .collect();

if !valid_targets.contains(&target.as_str()) { /* PreconditionFailed, exit */ }
```

When it is reachable:
- **Events appended:** one `DirectedTransition { from: P, to: P, rationale }` at `src/cli/mod.rs:3336-3345`, then — if the response carries details — one `InstructionsDelivered { state: P }` at `:3459`. No gate evaluation happens on this path at all (`let gate_results = std::collections::BTreeMap::new();`, `src/cli/mod.rs:3357`), so no `GateEvaluated`.
- **Does it count as a transition?** Yes, unconditionally and unavoidably: the append happens before dispatch and is never conditional on `from != to`. `advanced` is hardcoded `true` in the `dispatch_next(target, ..., true, ...)` call at `:3359`.
- **Does it hit the same predicate path as a conditional self-transition?** Yes — the same `instructions_delivered_this_occupancy` and the same combinator. The only difference is the payload variant (`DirectedTransition` vs `Transitioned`) and that the directed path constructs its event list in memory. Both variants are read identically by `occupancy_slice` (`src/engine/persistence.rs:1031-1033`), so one fix in the predicate covers both.

This is confirmed by the existing test `two_consecutive_directed_transitions_into_same_phase_both_carry` (`tests/instructions_delivery_test.rs:485-514`), which asserts today's deliver-again behaviour and will need inverting.

### 4. What a non-advancing tick appends, and how suppression works today

A non-advancing tick on P can append:
- `EvidenceSubmitted { state: P, fields, submitter_cwd }` — `src/cli/mod.rs:3815-3819`, only when `--with-data` was supplied and validated.
- `GateEvaluated { state: P, gate, output, outcome, timestamp }` — `src/engine/advance.rs:382-392`, one per non-overridden gate, on every tick the phase has gates. Overridden gates emit nothing (`src/engine/advance.rs:364`).
- Nothing at all when the phase has no gates and no evidence was submitted.
- No `Transitioned`, by definition of non-advancing.
- Then `InstructionsDelivered` if and only if the response ended up carrying details.

Suppression on the second such tick: the log has no new entry event, so `occupancy_slice` still starts at the original arrival into P, and the slice contains the first tick's `InstructionsDelivered { state: P }`. The `any()` at `src/engine/persistence.rs:1100-1105` matches it (position *and* name), `already_delivered` is `true`, `suppress` is `true`, `details` is stripped. Covered by `gate_blocked_first_tick_carries_and_repeat_omits` (`tests/instructions_delivery_test.rs:445-469`).

This behaviour is unchanged by the intended fix: a self-loop-aware predicate widens the slice backwards, and widening can only add more `InstructionsDelivered` candidates, never remove the one already in scope.

### 5. `koto status`

**It records nothing.** `handle_status` (`src/cli/mod.rs:4977-5172`) performs no `append_event` call anywhere in its body, takes no lock, and never calls the predicate or the combinator. The doc comment states it explicitly (`src/cli/mod.rs:5077-5080`):
```
/// This retrieval always returns the full instructions regardless of
/// the delivery rule `koto next` applies (PRD R10), and it appends
/// nothing: no delivery record, no other event, and no lock is taken
/// anywhere in this function.
```

**It does not *always* include `details`** — it includes them whenever they exist. `src/cli/mod.rs:5081-5118`:
```rust
if let Some(state) = current_template_state {
    if !state.terminal {
        ...
        response["directive"] = serde_json::json!(substitute(&state.directive));
        if !state.details.is_empty() {
            response["details"] = serde_json::json!(substitute(&state.details));
        }
```
Two absences, neither an error and neither delivery-related: the whole `directive`/`details`/`expects` trio is absent at a terminal phase, and `details` alone is absent when the phase declares none. Delivery history never enters the decision. `koto status` needs no insulation from this change and must not gain any.

### 6. Instruction pointers naming `koto status`

One emitted string, `src/cli/next_types.rs:165-166`:
```rust
pub const RECOVERY_POINTER: &str =
    "[koto] Lost context? `koto status <name>` returns this phase's directive/details/expects.\n\n";
```
Spliced via `with_directive_prefix` at two sites — `src/cli/mod.rs:3426-3430` (directed, gated on `target_template_state.details.is_empty()`) and `src/cli/mod.rs:4310-4314` (natural, gated on `final_template_state.details.is_empty()`).

Both gate on **whether the phase declares instructions, not on whether this response carries them**, which is deliberate (`src/cli/next_types.rs:159-164`, DESIGN "The one thing the pointer must not key on"). That is exactly the property the change relies on: a self-loop that now suppresses still ships the pointer, so a context-lost agent has a recovery route. **The pointer text needs no change and the gating must not be touched.** Its invariants are pinned by `tests/next_response_baseline.rs`-adjacent assertions at `src/cli/next_types.rs:963-1016` (under 150 chars, contains `koto status`, prefixes the five variants, no-ops on `Terminal`/`Error`).

Prose that describes the rule and will drift:
- `docs/guides/cli-usage.md:82` — "delivered once per occupancy of a state (each time the workflow enters it — **including a rewind, a self-transition, or a directed transition back into it**)". Directly contradicted by the change.
- `docs/guides/cli-usage.md:117` and `:302` — occupancy wording, and the `koto status` note. `:302` stays correct.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:96` — "omitted once delivered, until the workflow leaves and re-enters the state". Reads ambiguously after the change (a self-loop is a re-entry that no longer re-delivers); worth tightening.
- `CHANGELOG.md:20-32` — describes the shipped rule.
- `src/engine/persistence.rs:1000-1004` and `:1082-1085` — the `occupancy_slice` / predicate doc comments state the self-transition semantics explicitly.
- `docs/designs/current/DESIGN-inline-phase-details.md:220`, `:247-256`, `:359` — the design's own reasoning, including the passage that deliberately *rewrote* the AC the user has now reinstated (see Surprises).

## Implications

**The blast radius is asymmetric, and that decides the shape of the fix.** `occupancy_slice` has two consumers, and only one of them wants the new semantics. Editing the shared helper would silently change the blocked classification on both dashboard surfaces (`src/cli/dashboard_data.rs:458`, `src/workflows_surface/project.rs:183`): `latest_epoch_gate_failed` takes the *latest* `GateEvaluated` in the slice, so widening the slice backwards across a self-loop makes a session that has just self-looped and not yet re-evaluated gates inherit the pre-loop gate verdict. That is a real, user-visible regression in the "blocked" badge. **Recommendation: leave `occupancy_slice` alone and change `instructions_delivered_this_occupancy` only** — either by giving `occupancy_slice` a boundary-policy parameter, or by adding a sibling scan that skips entry events where `from == Some(to)`. The gate epoch and the delivery epoch stop being the same thing, and the doc comment at `src/engine/persistence.rs:1017-1019` ("Shared rather than copied so the predicates built on it ... cannot come to disagree about where an occupancy starts") needs to be rewritten to say why they now legitimately differ.

**No call site needs insulating; only one needs its comment rewritten.** `koto status` never consults the predicate. Batch mode never touches `details`. The advance loop never touches `details`. `--full` short-circuits ahead of the predicate. The two `koto next` paths already share one combinator, so a single predicate change lands on both correctly and simultaneously. The one thing that breaks is the load-bearing comment at `src/cli/mod.rs:3382-3402`, which argues the directed path's predicate call "provably evaluates to `false` on every call"; after the change it evaluates to `true` on a `--to P`-while-at-P, which is precisely the new behaviour. That comment must be replaced, not trimmed.

**All three payload variants carry `from`, so the fix is mechanical.** `Transitioned { from: Option<String>, .. }`, `DirectedTransition { from: String, .. }`, `Rewound { from: String, .. }`. The initial transition is `Transitioned { from: None, to: initial }`, which is never a self-transition, so the `None` case needs no special handling. The directed path's synthetic event already carries the real `from` (`src/cli/mod.rs:3336-3340`), so no plumbing changes there.

**Tests and fixtures that must be inverted or rewritten:**
- `src/engine/persistence.rs:2595-2609` — `instructions_delivered_resets_on_a_self_transition`, asserts today's semantics head-on.
- `src/engine/persistence.rs:2628-2650` — `instructions_delivered_resets_on_arrival_by_directed_transition`; its first half (`gather -> implement`) stays, its second half (`directed(4, "implement", "implement")` then `!delivered`) inverts.
- `tests/instructions_delivery_test.rs:358-381` — `self_transition_arrival_carries_details_again`.
- `tests/instructions_delivery_test.rs:483-514` — `two_consecutive_directed_transitions_into_same_phase_both_carry`.
- `tests/next_response_baseline.rs:361-369` and `tests/fixtures/next-response-baseline/instruction-free.json:85` — the `self-transition-arrival` sequence. The *bodies* are safe (the baseline template declares no `<!-- details -->` anywhere, so `details` never appears), but its description string "ending one occupancy and beginning another" and NOTES entry at `tests/next_response_baseline.rs:269` become wrong prose inside a committed fixture.
- Unaffected and worth keeping as regression anchors: `gate_blocked_first_tick_carries_and_repeat_omits`, `loop_back_arrival_at_previously_occupied_phase_carries_details_again`, `rewind_arrival_carries_details`, both `--full` tests.

## Surprises

**The design document already litigated this exact question and ruled the other way — in writing, and by rewriting the acceptance criterion.** `docs/designs/current/DESIGN-inline-phase-details.md:247-256`:

> A contradiction in the PRD was corrected. Its Definitions made a self-transition begin a new occupancy — so instructions must be delivered — while an acceptance criterion required a second consecutive directed transition into the same phase to omit them. ... The Definitions are normative and R3 is explicit, so the criterion was rewritten to test what it was plainly reaching for: a directed transition followed by a non-advancing tick.

So this is not an oversight PR #197 made; it is a documented resolution of a documented contradiction, now being reversed by the user's ruling. **Whatever lands must go back and amend that passage**, otherwise the design doc asserts the opposite of the code and the next reader re-derives the discarded answer. `docs/prds/PRD-inline-phase-details.md`'s Definitions section presumably still carries the "self-transition begins a new occupancy" wording that the design cites as normative; that is upstream of the design and likely needs amending too.

**A self-rewind is reachable and nobody has considered it.** `handle_rewind` (`src/cli/mod.rs:2031-2047`) sets `prev_state` from the `to` field of the *second-to-last* state-changing event. If the last two state-changing events are both entries into P — which is exactly what a self-loop produces — then `prev_state == P` and koto appends `Rewound { from: P, to: P }`. A naive `from == to` rule would suppress on that rewind, but the intended semantics list "Rewind into P from elsewhere: DELIVER" and say nothing about rewinding into P from P. Needs an explicit call.

**Three near-identical epoch-boundary scans exist, and only two of them share code.** `derive_overrides` (`src/engine/persistence.rs:803-815`) and `derive_last_gate_evaluated` (`:848-860`) each open-code the same backwards scan that `occupancy_slice` (`:1029-1041`) performs, rather than calling it. They will *not* follow a change to `occupancy_slice` — which is convenient here (gate overrides keep their existing self-loop-resets semantics) but means the codebase has three definitions of "epoch" that agree today only by coincidence.

**`derive_visit_counts` is now dead outside its own tests.** `src/engine/persistence.rs:981` is `pub` and exercised by four unit tests (`:2298-2366`), but the delivery rule that used to call it was replaced. Grep finds no production caller. Not this change's job, but it is the fossil of the predicate being replaced a second time.

**Batch mode is a genuine non-event.** Worth stating because the lead asked: the only `details` token in `src/cli/batch.rs` is a `TemplateState` field in a test fixture at line 3221. The scheduler runs after suppression and only writes envelope siblings.

## Open Questions

1. **Does a rewind `P -> P` deliver or suppress?** Reachable via `handle_rewind` after a self-loop (see Surprises). Needs a human ruling before the predicate is written, since a simple `from == to` check answers "suppress" by default.
2. **Should the self-loop-aware slice be a parameter on `occupancy_slice` or a separate scan?** Both keep `latest_epoch_gate_failed` intact. A parameter keeps one scan but makes the "shared so they cannot disagree" doc comment self-contradictory; a sibling function is more honest but adds a third near-duplicate scan to the two that already exist.
3. **Which upstream documents get amended, and by whom?** The design doc's "A contradiction in the PRD was corrected" passage and the PRD's Definitions both assert the semantics being reversed. Leaving them is not an option; whether they are edited in this change or superseded by a new design decision is a process call.
4. **Does `P -> Q -> P` within a single tick count as a self-loop?** The advance loop can chain multi-hop, so an arrival at P whose immediately preceding entry event is also P but with a different phase in between within the same tick is expressible. The `from == to` check on the entry event answers this correctly (`from == Q`, so it delivers), but nobody has written down that it is the intended answer.
5. **Should `koto next --to P` while already at P remain legal at all?** It is reachable only when the template declares `P -> P`. Once it stops delivering instructions, its remaining observable effect is a `DirectedTransition` event with a rationale — worth confirming that is still a use case someone wants, rather than an accident of the target validation at `src/cli/mod.rs:3304-3322`.

## Summary

The delivery decision has exactly two deciding call sites — `src/cli/mod.rs:3417` (directed) and `src/cli/mod.rs:4298` (natural) — both funnelling through one combinator (`src/cli/next_types.rs:392`), while `koto status`, batch mode, and the advance loop never consult the predicate at all, so no surface needs insulating and `--full` is untouched. The one real hazard is that `occupancy_slice` is shared with `latest_epoch_gate_failed`, which feeds the dashboard and `/workflows` blocked classification, so the fix must scope to `instructions_delivered_this_occupancy` rather than editing the shared helper; the directed path's "provably evaluates to `false` on every call" comment (`src/cli/mod.rs:3382-3402`) also stops being true and must be rewritten. The biggest open question is a rewind `P -> P` — reachable after any self-loop via `handle_rewind`, and a naive `from == to` rule silently answers "suppress" for a case the intended semantics never discussed.
