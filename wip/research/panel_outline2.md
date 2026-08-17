# Verdict: PASS

## Claim check

**1. Directed path: `koto next --to P` at P now suppresses.**

- "The event just appended is then a `DirectedTransition { from: P, to: P }`" —
  **verified**. `src/cli/mod.rs:3336-3340` builds the payload with
  `from: current_state.clone(), to: target.clone()`; when `--to P` is issued at
  P those are the same string. `src/cli/mod.rs:3404-3414` wraps that exact
  `payload.clone()` (and the same `directed_ts` persisted at 3344-3345) into the
  synthetic `Event` chained onto `events`. The synthetic is shaped the way the
  comment assumes: same payload, same timestamp, placeholder `seq`/`event_type`,
  `idempotency_hash: None`.
- "which does not open a delivery window" — **verified**.
  `src/engine/persistence.rs:1078-1080`: the `DirectedTransition` arm opens only
  when `to == current_state && (boundary.opens_on_self_entry() || from != current_state)`,
  and `Boundary::ArrivalFromElsewhere` returns `false` from `opens_on_self_entry`
  (`persistence.rs:1022`). `delivery_window` passes that boundary
  (`persistence.rs:1125-1127`).
- "the scan reaches back past it to the arrival that did" — **verified** for a
  log with a real arrival: `entry_slice`'s `.rev().find_map` (`persistence.rs:1072-1089`)
  skips the self-entry and matches the preceding `Transitioned`/`DirectedTransition`
  whose `from != P`, or any `Rewound { to: P }`.
- "and finds that arrival's delivery record. The answer is `true`" —
  **refuted as an unconditional claim**. See Finding 1.
- The synthetic-event-equivalence claim ("only inspects `.payload`") — **verified**:
  `entry_slice` matches on `&e.payload` only (`persistence.rs:1073`), and
  `instructions_delivered_this_window` likewise (`persistence.rs:1194-1199`).
  `persistence.rs:1057-1063` states the same contract from the callee side.
- `events` really is the tick-start read taken before the append — **verified**:
  read at `src/cli/mod.rs:3094`, directed append at `3345`, and no
  `append_event(&name, ...)` call exists between those two lines.

**2. "It answers `false` on every other directed transition" — verified, including
the previously-occupied-and-left case.**

For any directed transition with `from != to`, the synthetic event satisfies
`to == current_state && from != current_state`, so it opens the window at
`persistence.rs:1079`. It is the last element of `post_events` by construction
(`mod.rs:3404-3414` chains it onto the end), so `entry_slice` returns
`&events[idx + 1..]` = empty (`persistence.rs:1091`) and `.any(..)` is `false`
(`persistence.rs:1194`). A delivery record from an earlier visit to P falls
before the opener and is therefore out of scope — the "occupied earlier and
left" case answers `false` and delivers, as the comment says. The
whole-log fallback arm (`persistence.rs:1092`) is unreachable here because the
synthetic always opens.

**3. Natural path: a self-transition-only tick stays inside the arrival's window — verified,
including the double-self and P → Q → P cases.**

- Two self-transitions in one tick: both are `Transitioned { from: Some(P), to: P }`;
  neither opens under `ArrivalFromElsewhere` (`persistence.rs:1074-1077`, the
  `as_deref() != Some(current_state)` test), so the reverse scan passes both and
  lands on the arrival. The comment's "a tick whose only movement was a
  transition from this phase to itself" holds for N ≥ 1 self-transitions.
- P → Q → P: the newest entry naming P is `Transitioned { from: Some(Q), to: P }`,
  which opens. The window is then only this tick's post-transition events. No
  `InstructionsDelivered` can be inside it — both sites append the record after
  `println!` (`mod.rs:4596-4618`), i.e. after the `read_events` at `mod.rs:4299-4302` —
  so the predicate answers `false` and the response delivers. The comment's
  second half ("a tick that left and came back from anywhere else opened a new
  one and delivers") is exact.

**4. `InstructionsDelivered` doc comment — both halves verified.**

- "Appended by both `koto next` response-construction sites, after the response
  has been printed, when it carried the phase's instructions": directed path
  prints at `mod.rs:3450` and appends at `mod.rs:3455-3466` under
  `if resp.carries_details()`; natural path prints at `mod.rs:4596-4599` and
  appends at `mod.rs:4607-4618` under the same guard.
  `carries_details` is `details.is_some()` on the five detail-bearing variants
  and `false` for `Terminal`/`Error` (`src/cli/next_types.rs:492-501`), so "when
  it carried the phase's instructions" is accurate.
- "a crash between printing and appending re-delivers on the next tick, which is
  the benign direction": with no record written, the next tick's window holds no
  `InstructionsDelivered` for the phase, so `instructions_delivered_this_window`
  returns `false` (`persistence.rs:1194-1199`) and details ship again. Matches the
  predicate's own doc (`persistence.rs:1174-1179`) and the in-code rationale at
  `mod.rs:4600-4606`. Both appends are non-fatal (`eprintln!` warning), so the
  crash window is the only way to lose the record — consistent with the claim.

**5. Reads — verified, neither path gains nor loses one.**

The directed hunk changes only comment lines plus `instructions_delivered_this_occupancy`
→ `instructions_delivered_this_window` at `mod.rs:3415`; the in-memory
`post_events` construction at `mod.rs:3404-3414` is untouched by the diff. The
natural hunk changes only comment lines plus the same rename at `mod.rs:4303`;
the single `backend.read_events(&name)` at `mod.rs:4299-4302` is unchanged. No
`read_events` call is added or removed anywhere in the diff.

**6. Scope — verified.**

Three files only: `src/cli/mod.rs`, `src/cli/next_types.rs`, `src/engine/types.rs`.
Inside them, every non-comment change is the import rename (`mod.rs:2914`) and
the two call-site renames (`mod.rs:3415`, `mod.rs:4303`). `next_types.rs` is
doc-comment-only (3 lines). `src/engine/types.rs` is doc-comment-only (8 lines
inside the `InstructionsDelivered` rustdoc): no `EventPayload` variant added, no
field added to the delivery event, and `CURRENT_SCHEMA_VERSION` (`types.rs:199`)
is untouched.

**7. Terminal-variant unit tests — verified unmodified.**

`git show 746caf1 -- src/cli/next_types.rs` contains a single hunk at line 371,
the `with_details_suppressed_unless_full` rustdoc. The three tests are outside
it and their assertions are as they were:
`recovery_pointer_prefix_leaves_terminal_and_error_unchanged` (`next_types.rs:993`),
`suppress_terminal_and_error_pass_through_unchanged` (`next_types.rs:1409`),
`carries_details_false_for_terminal_and_error` (`next_types.rs:1453`).

## Findings

**1. MAJOR (comment precision, no code hazard) — the directed path's comment
states an unconditional `true` the predicate does not guarantee.**

`src/cli/mod.rs:3385-3389`:

> "so the scan reaches back past it to the arrival that did and finds that
> arrival's delivery record. The answer is `true` and the instructions are
> suppressed"

The answer is `true` only when the arrival's window actually holds an
`InstructionsDelivered { state: P }`. Two reachable logs where it does not:

- A crash lands between `println!` and the record append. The predicate's own
  doc comment names this case explicitly and treats it as the reason the rule is
  keyed on a *recorded* delivery rather than on the shape of the entry event
  (`src/engine/persistence.rs:1174-1179`). In that log the self-directed tick
  answers `false` and re-delivers — which is the intended behavior, and the
  opposite of what this comment asserts.
- The workflow stands at P because `koto init` wrote
  `Transitioned { from: None, to: P }` and no `koto next` has run yet. If the
  template declares a P → P transition (the precondition for `--to P` passing
  validation at `mod.rs:3311-3322`), the first command can be `koto next --to P`.
  The window is the whole post-init slice, it holds no delivery record, and the
  answer is `false`.

Additionally, "the scan reaches back past it to the arrival that did" presumes a
qualifying arrival exists. Under `ArrivalFromElsewhere` a log whose only entries
naming P are self-entries falls through to the whole-log arm
(`persistence.rs:1090-1093`), which `persistence.rs:1052-1055` documents as the
one place this boundary's failure direction inverts. The comment does not
acknowledge that arm.

This is the same *kind* of defect as the comment it replaces — an unconditional
statement about the predicate's answer — but materially smaller in consequence:
the replacement correctly says the call is decision-bearing and that both
answers occur, so nothing invites deleting the call or hardcoding a constant.
No code depends on the claim. It is worth fixing because a reader who takes it
literally would write an integration assertion ("self-directed transition always
suppresses") that is false on a crash-recovered log.

Fix — soften two clauses:

> ...so the scan reaches back past it to the arrival that did. When that arrival
> recorded a delivery, the answer is `true` and the instructions are suppressed —
> a hand-driven lap of a declared loop is not an arrival. When it did not (a
> crash between printing a response and appending its record), the window holds
> no record and koto re-delivers, which is the direction the design accepts.

**2. INFORMATIONAL — stale terminology left in a test comment, not this
commit's.**

`src/engine/persistence.rs:2791` still reads "*inside* the current occupancy".
That file is not touched by 746caf1, so it is residue from outline 1
(9532a95/afd5983) rather than a defect here. Outline 4's sweep should catch it;
noting so it is not lost. Every other `occupancy` reference in the tree is in
docs (`DESIGN-inline-phase-details.md`, `PRD-inline-phase-details.md`, the plan
and design for this change), i.e. prior-artifact text, not live code.

**3. INFORMATIONAL — the commit does not build the tree, by design.**

The plan's own criterion (`PLAN-self-loop-suppresses-details.md:137-143`) sets
the bar at `cargo test --lib`, notes three integration assertions still encode
the old rule until outline 3, and says `cargo test` is first green at the end of
outline 3. Per the panel's constraints I did not run any build or test; the
scope and rename verification above is by reading. The `use` at `mod.rs:2914`
and the two call sites are the only references to the renamed symbol in
`src/`, and `instructions_delivered_this_window` exists as `pub fn` at
`persistence.rs:1193`, so the rename is complete as far as static reading shows.

## Summary

The three code edits are correct and minimal: the import and both call sites now
reach `instructions_delivered_this_window`, no read is added or removed on either
path, and `src/engine/types.rs` gains no variant, no field, and no schema-version
change. Claims 2, 3, 4, 5, 6, and 7 all check out exactly — notably the
"`false` on every other directed transition" half is airtight including the
previously-occupied-and-left case, and the natural path's window claim holds for
double self-transitions and for P → Q → P.

The one soft spot is the directed path's assertion that the self-directed case
yields `true` and "finds that arrival's delivery record". That is the common case,
not a guarantee: a crash between printing and appending, or a `--to P` issued at
the init state before any `koto next`, leaves the window with no record and the
answer is `false`. The predicate's own doc comment at persistence.rs:1174-1179
names the first of those explicitly, so the two comments now disagree.

Not blocking — the replacement comment correctly frames the call as
decision-bearing and no code depends on the overclaim — but the two clauses
should be softened before merge so the file does not again carry a statement
stronger than the code behind it.
